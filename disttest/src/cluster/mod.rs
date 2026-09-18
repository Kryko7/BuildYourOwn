//! A cluster of nodes, the proxies in front of them, and everything a cluster stage does to
//! it: find the leader, kill a member, cut the network, heal it, wait for the others to
//! agree again.
//!
//! Members are numbered from zero and named `m1`, `m2`, ... Each one gets three ports: the
//! client port it serves requests on, the private peer port it really listens on, and the
//! *proxy* port every other member is told to use. Client traffic goes straight to the
//! member; peer traffic goes through [`proxy`], which is what makes partitions possible.

pub mod proxy;
pub mod workload;

use crate::assert::{Failure, FailureKind};
use crate::config::TargetDef;
use crate::etcd::Client;
use crate::node::{free_ports, spec_for, NodeHandle};
use proxy::{FaultTable, LinkFault, PeerProxy, ProxyStats, SourceMap};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One member: its process, its client, and the ports it was given.
pub struct MemberNode {
    /// Index in the cluster, from zero.
    pub index: usize,
    /// The member name (`m1`, ...).
    pub name: String,
    /// The process, absent while the member is stopped.
    pub node: Option<NodeHandle>,
    /// A client bound to this member's client URL.
    pub client: Client,
    /// The port clients use.
    pub client_port: u16,
    /// The port the member really listens on for peer traffic.
    pub peer_port: u16,
    /// The port other members are told to use.
    pub proxy_port: u16,
    /// This member's data directory.
    pub data_dir: PathBuf,
    /// The member id it reported, once it has answered a status request.
    pub member_id: u64,
}

impl MemberNode {
    /// True while the process is running.
    pub fn running(&mut self) -> bool {
        self.node.as_mut().is_some_and(NodeHandle::alive)
    }

    /// The peer URL other members dial (the proxy).
    pub fn advertised_peer_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.proxy_port)
    }
}

/// A running cluster.
pub struct Cluster {
    /// The members, in index order. Spare slots (for membership changes) are at the end and
    /// start out stopped.
    pub members: Vec<MemberNode>,
    /// How many members the cluster was formed with.
    pub initial_size: usize,
    /// The link faults every proxy reads.
    pub faults: Arc<FaultTable>,
    /// What the proxies have done so far.
    pub stats: Arc<ProxyStats>,
    sources: Arc<SourceMap>,
    proxies: Vec<PeerProxy>,
    def: TargetDef,
    dist: Option<PathBuf>,
    tmp: PathBuf,
    timeout: Duration,
    /// Set by anything that changes topology or injects a fault, so the runner knows the
    /// cluster cannot be handed to the next test.
    pub dirty: bool,
}

impl Cluster {
    /// Start a cluster of `size` members, with `spare` further members configured but not
    /// started (stage 48 adds one of them).
    #[allow(clippy::too_many_arguments)]
    pub async fn start(
        def: &TargetDef,
        dist: Option<&Path>,
        tmp: &Path,
        size: usize,
        spare: usize,
        timeout: Duration,
        seed: u64,
    ) -> Result<Cluster, Failure> {
        let total = size + spare;
        let ports = free_ports(total * 3)
            .map_err(|e| Failure::harness(format!("cannot allocate ports: {e:#}")))?;
        let faults = Arc::new(FaultTable::default());
        let sources = Arc::new(SourceMap::default());
        let stats = Arc::new(ProxyStats::default());
        let mut members = Vec::new();
        let mut proxies = Vec::new();
        for i in 0..total {
            let (client_port, peer_port, proxy_port) =
                (ports[i * 3], ports[i * 3 + 1], ports[i * 3 + 2]);
            let name = format!("m{}", i + 1);
            let data_dir = tmp.join(&name).join("data");
            let client = Client::new(&format!("http://127.0.0.1:{client_port}"), &name, timeout)
                .map_err(|e| Failure::harness(format!("cannot build a client for {name}: {e}")))?;
            proxies.push(
                PeerProxy::start(
                    i,
                    proxy_port,
                    peer_port,
                    faults.clone(),
                    sources.clone(),
                    stats.clone(),
                    seed ^ (i as u64 + 1),
                )
                .await
                .map_err(|e| {
                    Failure::harness(format!("cannot start the peer proxy for {name}: {e}"))
                })?,
            );
            members.push(MemberNode {
                index: i,
                name,
                node: None,
                client,
                client_port,
                peer_port,
                proxy_port,
                data_dir,
                member_id: 0,
            });
        }
        let mut cluster = Cluster {
            members,
            initial_size: size,
            faults,
            stats,
            sources,
            proxies,
            def: def.clone(),
            dist: dist.map(Path::to_path_buf),
            tmp: tmp.to_path_buf(),
            timeout,
            dirty: false,
        };
        let initial = cluster.initial_cluster(size);
        for i in 0..size {
            cluster.spawn_member(i, &initial, "new")?;
        }
        for i in 0..size {
            cluster.wait_member_ready(i)?;
        }
        cluster.learn_member_ids().await;
        Ok(cluster)
    }

    /// The `--initial-cluster` string listing the first `n` members by their proxy URLs.
    pub fn initial_cluster(&self, n: usize) -> String {
        self.members
            .iter()
            .take(n)
            .map(|m| format!("{}={}", m.name, m.advertised_peer_url()))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Start one member's process from a given cluster view.
    fn spawn_member(
        &mut self,
        i: usize,
        initial_cluster: &str,
        state: &str,
    ) -> Result<(), Failure> {
        let m = &self.members[i];
        let mut spec = spec_for(
            &self.def,
            self.dist.as_deref(),
            &m.name,
            &self.tmp.join(&m.name),
            &m.data_dir,
            m.client_port,
            m.peer_port,
            initial_cluster,
            &m.advertised_peer_url(),
        )
        .map_err(|e| Failure::harness(format!("cannot describe member {}: {e:#}", m.name)))?;
        if state != "new" {
            spec.argv.push("--initial-cluster-state".to_string());
            spec.argv.push(state.to_string());
        }
        spec.boot_timeout = self.timeout.max(Duration::from_millis(20_000));
        let handle = NodeHandle::spawn(spec)
            .map_err(|e| Failure::harness(format!("cannot start member {}: {e:#}", m.name)))?;
        if let Some(pid) = handle.pid() {
            self.sources.register(i, pid);
        }
        self.members[i].node = Some(handle);
        self.sources.forget();
        Ok(())
    }

    fn wait_member_ready(&mut self, i: usize) -> Result<(), Failure> {
        let name = self.members[i].name.clone();
        let Some(node) = self.members[i].node.as_mut() else {
            return Err(Failure::harness(format!("member {name} was never started")));
        };
        node.wait_until_ready()
            .map_err(|e| Failure::harness(format!("member {name} never came up: {e:#}")))
    }

    /// Ask every running member who it is, so the leader id can be turned into an index.
    pub async fn learn_member_ids(&mut self) {
        for i in 0..self.members.len() {
            if self.members[i].node.is_none() {
                continue;
            }
            if let Ok(s) = self.members[i].client.status().await {
                self.members[i].member_id = s.header.member_id;
            }
        }
    }

    /// The client of one member.
    pub fn client(&self, i: usize) -> &Client {
        &self.members[i].client
    }

    /// Every index that currently has a running process.
    pub fn running(&mut self) -> Vec<usize> {
        (0..self.members.len())
            .filter(|i| self.members[*i].running())
            .collect()
    }

    /// The indices of the members the cluster was formed with.
    pub fn voters(&self) -> Vec<usize> {
        (0..self.initial_size).collect()
    }

    /// Which member index a member id belongs to.
    pub fn index_of_id(&self, id: u64) -> Option<usize> {
        self.members
            .iter()
            .position(|m| m.member_id != 0 && m.member_id == id)
    }

    /// Ask one member who it thinks the leader is.
    pub async fn leader_according_to(&self, i: usize) -> Option<usize> {
        let s = self.members[i].client.status().await.ok()?;
        if s.leader == 0 {
            return None;
        }
        self.index_of_id(s.leader)
    }

    /// Wait until every running member names the same leader.
    ///
    /// Returns the leader's index. This is the only way a cluster stage should look for a
    /// leader: asking one member can catch it mid-election.
    pub async fn wait_for_leader(&mut self, within: Duration) -> Result<usize, Failure> {
        let deadline = Instant::now() + within;
        let mut last;
        loop {
            let running = self.running();
            let mut leaders: Vec<Option<usize>> = Vec::new();
            for i in &running {
                leaders.push(self.leader_according_to(*i).await);
            }
            // A leader that has just been killed is still named by everyone for about a
            // second, because nobody has missed a heartbeat yet. Handing that index back
            // would send the next request to a closed port, so a leader is only agreed when
            // it is also still running.
            let agreed = leaders
                .first()
                .copied()
                .flatten()
                .filter(|l| leaders.iter().all(|x| *x == Some(*l)))
                .filter(|l| running.contains(l));
            if let Some(l) = agreed {
                return Ok(l);
            }
            last = format!(
                "members {:?} name leaders {:?}",
                running.iter().map(|i| i + 1).collect::<Vec<_>>(),
                leaders.iter().map(|l| l.map(|x| x + 1)).collect::<Vec<_>>()
            );
            if Instant::now() >= deadline {
                return Err(Failure::new(
                    FailureKind::Assertion,
                    format!(
                        "the cluster did not agree on one leader within {} ms",
                        within.as_millis()
                    ),
                )
                .note(last)
                .note(self.faults.describe()));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Wait until every running member has caught up to `revision`.
    pub async fn wait_for_revision(
        &mut self,
        revision: i64,
        within: Duration,
    ) -> Result<(), Failure> {
        let deadline = Instant::now() + within;
        loop {
            let mut behind = Vec::new();
            for i in self.running() {
                match self.members[i].client.status().await {
                    Ok(s) if s.header.revision >= revision => {}
                    Ok(s) => behind.push(format!(
                        "{} at revision {}",
                        self.members[i].name, s.header.revision
                    )),
                    Err(e) => behind.push(format!("{}: {e}", self.members[i].name)),
                }
            }
            if behind.is_empty() {
                return Ok(());
            }
            if Instant::now() >= deadline {
                return Err(Failure::new(
                    FailureKind::Assertion,
                    format!(
                        "not every member reached revision {revision} within {} ms",
                        within.as_millis()
                    ),
                )
                .note(behind.join("; ")));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    // -----------------------------------------------------------------------------------
    // Faults
    // -----------------------------------------------------------------------------------

    /// Cut every link crossing the given groups.
    pub async fn partition(&mut self, groups: &[Vec<usize>]) {
        self.dirty = true;
        for (a, b) in proxy::partition_links(groups) {
            self.faults.set(
                a,
                b,
                LinkFault {
                    cut: true,
                    ..self.faults.get(a, b)
                },
            );
        }
        self.cut_unattributable();
        self.forget_sources();
        self.reset_clients().await;
    }

    /// Cut a member off from every other member.
    pub async fn isolate(&mut self, i: usize) {
        self.dirty = true;
        for j in 0..self.members.len() {
            if j == i {
                continue;
            }
            self.faults.set(
                i,
                j,
                LinkFault {
                    cut: true,
                    ..self.faults.get(i, j)
                },
            );
        }
        // The destination's own link also stands for "a dialer the harness could not
        // identify", so an isolated member is isolated even without /proc.
        self.faults.set(
            i,
            i,
            LinkFault {
                cut: true,
                ..Default::default()
            },
        );
        self.forget_sources();
        self.reset_clients().await;
    }

    /// Refuse every connection whose dialer the harness could not identify.
    ///
    /// A member's own link stands for "somebody, we could not tell who". While the network
    /// is whole that is harmless and the connection is waved through; while a partition
    /// stands it is the one hole a cut could leak through, because an unattributable
    /// connection might be crossing the cut. Refusing it is the conservative reading, and a
    /// peer transport that loses a connection simply redials.
    fn cut_unattributable(&self) {
        for i in 0..self.members.len() {
            self.faults.set(
                i,
                i,
                LinkFault {
                    cut: true,
                    ..self.faults.get(i, i)
                },
            );
        }
    }

    /// Put one fault on one link.
    pub async fn link_fault(&mut self, a: usize, b: usize, fault: LinkFault) {
        self.dirty = true;
        self.faults.set(a, b, fault);
        self.forget_sources();
    }

    /// Put the same fault on every link.
    pub async fn every_link(&mut self, fault: LinkFault) {
        self.dirty = true;
        for i in 0..self.members.len() {
            for j in (i + 1)..self.members.len() {
                self.faults.set(i, j, fault.clone());
            }
        }
        self.forget_sources();
    }

    /// Remove every fault.
    pub async fn heal(&mut self) {
        self.faults.clear();
        self.forget_sources();
        self.reset_clients().await;
    }

    async fn reset_clients(&self) {
        for m in &self.members {
            m.client.reset().await;
        }
    }

    /// Throw away every cached "which member opened this connection" answer.
    ///
    /// Called whenever the fault table changes. The lookup is keyed on a pair of ports, and
    /// a blocked link churns connections fast enough to recycle ephemeral ports, so a cache
    /// that outlives a fault change is the one way a cut link could quietly let something
    /// through. Re-identifying costs one `/proc` scan per new connection.
    fn forget_sources(&self) {
        self.sources.forget();
    }

    // -----------------------------------------------------------------------------------
    // Process control
    // -----------------------------------------------------------------------------------

    /// Kill one member with `SIGKILL`: a power cut, nothing flushed.
    pub async fn kill(&mut self, i: usize) {
        self.dirty = true;
        if let Some(node) = self.members[i].node.as_mut() {
            node.kill_hard();
        }
        self.members[i].node = None;
        self.members[i].client.reset().await;
        self.sources.forget();
    }

    /// Stop one member politely.
    pub async fn stop(&mut self, i: usize) {
        self.dirty = true;
        if let Some(node) = self.members[i].node.as_mut() {
            node.stop();
        }
        self.members[i].node = None;
        self.members[i].client.reset().await;
        self.sources.forget();
    }

    /// Start a member again from its own data directory.
    pub async fn start_member(&mut self, i: usize) -> Result<(), Failure> {
        self.dirty = true;
        let initial = self.initial_cluster(self.initial_size);
        // Restarting a member of a formed cluster is an `existing` start: the data
        // directory already holds the cluster it belongs to.
        self.spawn_member(i, &initial, "existing")?;
        self.wait_member_ready(i)?;
        self.members[i].client.reset().await;
        if let Ok(s) = self.members[i].client.status().await {
            self.members[i].member_id = s.header.member_id;
        }
        Ok(())
    }

    /// Add a spare member to the cluster through `/v3/cluster/member/add`, then start it.
    pub async fn add_member(&mut self, i: usize, through: usize) -> Result<(), Failure> {
        self.dirty = true;
        let url = self.members[i].advertised_peer_url();
        self.members[through]
            .client
            .member_add(std::slice::from_ref(&url))
            .await
            .map_err(|e| {
                Failure::new(FailureKind::Assertion, format!("member add failed: {e}")).note(
                    format!("adding {url} through {}", self.members[through].name),
                )
            })?;
        // The joining member is told about every member the cluster was formed with, plus
        // itself; anything else and its own view of the configuration would not match the
        // one the leader just wrote.
        let mut view: Vec<String> = (0..self.initial_size)
            .map(|j| {
                format!(
                    "{}={}",
                    self.members[j].name,
                    self.members[j].advertised_peer_url()
                )
            })
            .collect();
        view.push(format!("{}={}", self.members[i].name, url));
        self.spawn_member(i, &view.join(","), "existing")?;
        self.wait_member_ready(i)?;
        if let Ok(s) = self.members[i].client.status().await {
            self.members[i].member_id = s.header.member_id;
        }
        Ok(())
    }

    /// Remove a member through `/v3/cluster/member/remove` and stop its process.
    pub async fn remove_member(&mut self, i: usize, through: usize) -> Result<(), Failure> {
        self.dirty = true;
        let id = self.members[i].member_id;
        self.members[through]
            .client
            .member_remove(id)
            .await
            .map_err(|e| {
                Failure::new(FailureKind::Assertion, format!("member remove failed: {e}")).note(
                    format!(
                        "removing {} ({id:#x}) through {}",
                        self.members[i].name, self.members[through].name
                    ),
                )
            })?;
        self.stop(i).await;
        Ok(())
    }

    /// Stop every member, then start them all again from their own data directories.
    pub async fn restart_all(&mut self) -> Result<(), Failure> {
        self.dirty = true;
        let running = self.running();
        for i in &running {
            self.stop(*i).await;
        }
        let initial = self.initial_cluster(self.initial_size);
        for i in &running {
            self.spawn_member(*i, &initial, "existing")?;
        }
        for i in &running {
            self.wait_member_ready(*i)?;
        }
        self.reset_clients().await;
        self.learn_member_ids().await;
        Ok(())
    }

    /// The output of every member, for a failure block.
    pub fn output(&self) -> String {
        let mut s = String::new();
        for m in &self.members {
            if let Some(n) = &m.node {
                s.push_str(&n.output_tail(8));
                s.push('\n');
            }
        }
        s.trim_end().to_string()
    }

    /// A one-line description of who is running and who leads.
    pub async fn describe(&mut self) -> String {
        let mut parts = Vec::new();
        for i in 0..self.members.len() {
            let name = self.members[i].name.clone();
            if !self.members[i].running() {
                if i < self.initial_size {
                    parts.push(format!("{name} stopped"));
                }
                continue;
            }
            match self.members[i].client.status().await {
                Ok(s) => parts.push(format!(
                    "{name} rev {} term {} leader {}",
                    s.header.revision,
                    s.raft_term,
                    self.index_of_id(s.leader)
                        .map(|l| format!("m{}", l + 1))
                        .unwrap_or_else(|| "none".into())
                )),
                Err(e) => parts.push(format!("{name} unreachable ({e})")),
            }
        }
        parts.join(" | ")
    }

    /// Stop every process and every proxy. Called on every path out of a test.
    pub fn shutdown(&mut self) {
        for m in &mut self.members {
            if let Some(mut n) = m.node.take() {
                n.stop();
            }
        }
        for p in &self.proxies {
            p.stop();
        }
    }

    /// True when a test left the cluster in a state the next test cannot reuse.
    pub fn is_dirty(&mut self) -> bool {
        if self.dirty {
            return true;
        }
        (0..self.initial_size).any(|i| !self.members[i].running())
    }
}

impl Drop for Cluster {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// Which member ids the cluster believes exist, as seen by one member.
pub async fn member_ids(client: &Client) -> HashMap<u64, String> {
    match client.member_list().await {
        Ok(list) => list.members.into_iter().map(|m| (m.id, m.name)).collect(),
        Err(_) => HashMap::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_member_advertises_its_proxy_not_its_own_port() {
        let m = MemberNode {
            index: 0,
            name: "m1".into(),
            node: None,
            client: Client::new("http://127.0.0.1:1", "m1", Duration::from_secs(1))
                .expect("client"),
            client_port: 1,
            peer_port: 2,
            proxy_port: 3,
            data_dir: PathBuf::from("/tmp"),
            member_id: 0,
        };
        assert_eq!(m.advertised_peer_url(), "http://127.0.0.1:3");
    }
}
