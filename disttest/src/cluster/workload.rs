//! The randomized workload and the fault schedule the cluster stages run it under.
//!
//! Several client tasks hammer a handful of keys through *different members* while a
//! seeded schedule cuts the network, kills processes and heals again. Every call is
//! recorded with the instant it was made and the instant the answer arrived, so the result
//! is a history the linearizability checker can decide.
//!
//! Everything is a function of `--seed`: which member each call goes to, which key, which
//! operation, which value, and when each fault fires. Two runs with the same seed issue the
//! same calls in the same order at the same planned times.

use crate::assert::Failure;
use crate::cluster::proxy::LinkFault;
use crate::cluster::Cluster;
use crate::etcd::{self, Client};
use crate::lin::{Entry, History, Op, Outcome, Recorder};
use rand::rngs::StdRng;
use rand::{Rng, SeedableRng};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// What the workload should do.
#[derive(Debug, Clone)]
pub struct WorkloadSpec {
    /// How many client tasks run at once.
    pub clients: usize,
    /// How many keys they share.
    pub keys: usize,
    /// How long the workload runs.
    pub duration: Duration,
    /// Deadline for one call; a call that passes it is recorded as "no answer".
    pub op_timeout: Duration,
    /// How long a client waits between calls.
    pub pause: Duration,
    /// The run's seed.
    pub seed: u64,
    /// Prefix every key of this workload carries, so stages never collide.
    pub prefix: String,
}

impl WorkloadSpec {
    /// A small workload: four clients, three keys, a few seconds.
    pub fn small(seed: u64, prefix: &str) -> WorkloadSpec {
        WorkloadSpec {
            clients: 4,
            keys: 3,
            duration: Duration::from_millis(4_000),
            op_timeout: Duration::from_millis(1_500),
            pause: Duration::from_millis(15),
            seed,
            prefix: prefix.to_string(),
        }
    }

    /// The key names this workload uses.
    pub fn key_names(&self) -> Vec<String> {
        (0..self.keys)
            .map(|i| format!("{}/k{i}", self.prefix))
            .collect()
    }
}

/// What the workload saw.
#[derive(Debug, Default)]
pub struct WorkloadResult {
    /// Every operation, with its window.
    pub history: History,
    /// Operations that were acknowledged.
    pub acknowledged: usize,
    /// Operations whose answer never arrived.
    pub unknown: usize,
    /// How many calls each member served.
    pub per_member: Vec<usize>,
    /// What the fault schedule actually did, in order.
    pub faults_applied: Vec<String>,
}

impl WorkloadResult {
    /// A line for `ctx.note`.
    pub fn summary(&self) -> String {
        format!(
            "{} operations ({} acknowledged, {} with no answer) over {} keys; per member {:?}",
            self.history.len(),
            self.acknowledged,
            self.unknown,
            self.history.keys().len(),
            self.per_member
        )
    }
}

/// One thing the schedule does to the cluster.
#[derive(Debug, Clone, PartialEq)]
pub enum FaultEvent {
    /// Cut the cluster into groups.
    Partition(Vec<Vec<usize>>),
    /// Cut one member off from everyone.
    Isolate(usize),
    /// `SIGKILL` the member that currently leads.
    KillLeader,
    /// `SIGKILL` one member.
    Kill(usize),
    /// Start every stopped member again.
    RestartStopped,
    /// Delay every link.
    DelayAll(Duration),
    /// Refuse new peer connections with this probability.
    LossyAll(f64),
    /// Duplicate and reorder framed peer messages.
    MangleAll {
        /// Probability a message is sent twice.
        duplicate: f64,
        /// Probability a message is held back one slot.
        reorder: f64,
    },
    /// Remove every fault.
    Heal,
}

impl FaultEvent {
    /// How the report names this event.
    pub fn label(&self) -> String {
        match self {
            FaultEvent::Partition(groups) => format!(
                "partition {}",
                groups
                    .iter()
                    .map(|g| format!(
                        "{{{}}}",
                        g.iter().map(|i| format!("m{}", i + 1)).collect::<Vec<_>>().join(",")
                    ))
                    .collect::<Vec<_>>()
                    .join(" | ")
            ),
            FaultEvent::Isolate(i) => format!("isolate m{}", i + 1),
            FaultEvent::KillLeader => "kill the leader".to_string(),
            FaultEvent::Kill(i) => format!("kill m{}", i + 1),
            FaultEvent::RestartStopped => "restart whatever is stopped".to_string(),
            FaultEvent::DelayAll(d) => format!("delay every link by {} ms", d.as_millis()),
            FaultEvent::LossyAll(p) => format!("drop {:.0}% of peer connections", p * 100.0),
            FaultEvent::MangleAll { duplicate, reorder } => format!(
                "duplicate {:.0}% and reorder {:.0}% of peer messages",
                duplicate * 100.0,
                reorder * 100.0
            ),
            FaultEvent::Heal => "heal".to_string(),
        }
    }
}

/// A seeded plan of faults over the life of a workload.
#[derive(Debug, Clone, Default)]
pub struct FaultSchedule {
    /// When each event fires, measured from the start of the workload.
    pub events: Vec<(Duration, FaultEvent)>,
}

impl FaultSchedule {
    /// No faults at all.
    pub fn none() -> FaultSchedule {
        FaultSchedule::default()
    }

    /// Exactly these events.
    pub fn of(events: Vec<(Duration, FaultEvent)>) -> FaultSchedule {
        FaultSchedule { events }
    }

    /// A seeded schedule over `duration` for a cluster of `members`.
    ///
    /// The plan alternates one fault and one recovery, and always ends healed with every
    /// member running, so the final read-back has a cluster to answer it.
    pub fn random(seed: u64, members: usize, duration: Duration) -> FaultSchedule {
        let mut rng = StdRng::seed_from_u64(seed ^ 0x_fa17_5eed);
        let mut events = Vec::new();
        let majority = members / 2 + 1;
        let rounds = 3;
        let slot = duration.as_millis() as u64 / (rounds * 2 + 1).max(1);
        for round in 0..rounds {
            let at = Duration::from_millis(slot * (round * 2 + 1));
            let choice = rng.random_range(0..5);
            let event = match choice {
                0 => {
                    let victim = rng.random_range(0..members);
                    FaultEvent::Isolate(victim)
                }
                1 => {
                    // A minority and a majority, chosen by shuffling the members.
                    let mut idx: Vec<usize> = (0..members).collect();
                    for i in (1..idx.len()).rev() {
                        idx.swap(i, rng.random_range(0..=i));
                    }
                    let (minority, rest) = idx.split_at(members - majority);
                    FaultEvent::Partition(vec![minority.to_vec(), rest.to_vec()])
                }
                2 => FaultEvent::KillLeader,
                3 => FaultEvent::DelayAll(Duration::from_millis(rng.random_range(20..120))),
                _ => FaultEvent::LossyAll(0.10),
            };
            events.push((at, event));
            events.push((
                Duration::from_millis(slot * (round * 2 + 2)),
                FaultEvent::Heal,
            ));
            events.push((
                Duration::from_millis(slot * (round * 2 + 2) + 1),
                FaultEvent::RestartStopped,
            ));
        }
        FaultSchedule { events }
    }

    /// The events in the order they fire.
    pub fn sorted(&self) -> Vec<(Duration, FaultEvent)> {
        let mut e = self.events.clone();
        e.sort_by_key(|(at, _)| *at);
        e
    }

    /// A human-readable plan, for the report.
    pub fn render(&self) -> String {
        if self.events.is_empty() {
            return "no faults".into();
        }
        self.sorted()
            .iter()
            .map(|(at, e)| format!("+{:>5} ms  {}", at.as_millis(), e.label()))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Apply one event to a cluster.
pub async fn apply(cluster: &mut Cluster, event: &FaultEvent) -> Result<String, Failure> {
    match event {
        FaultEvent::Partition(groups) => cluster.partition(groups).await,
        FaultEvent::Isolate(i) => cluster.isolate(*i).await,
        FaultEvent::Kill(i) => cluster.kill(*i).await,
        FaultEvent::KillLeader => {
            let leader = cluster.wait_for_leader(Duration::from_millis(3_000)).await;
            match leader {
                Ok(l) => {
                    cluster.kill(l).await;
                    return Ok(format!("kill the leader (m{})", l + 1));
                }
                Err(_) => return Ok("kill the leader (no leader to kill)".into()),
            }
        }
        FaultEvent::RestartStopped => {
            let stopped: Vec<usize> = (0..cluster.initial_size)
                .filter(|i| !cluster.members[*i].running())
                .collect();
            for i in &stopped {
                cluster.start_member(*i).await?;
            }
            return Ok(format!("restarted {} member(s)", stopped.len()));
        }
        FaultEvent::DelayAll(d) => {
            cluster
                .every_link(LinkFault {
                    delay: *d,
                    ..Default::default()
                })
                .await
        }
        FaultEvent::LossyAll(p) => {
            cluster
                .every_link(LinkFault {
                    drop_connection: *p,
                    ..Default::default()
                })
                .await
        }
        FaultEvent::MangleAll { duplicate, reorder } => {
            cluster
                .every_link(LinkFault {
                    duplicate: *duplicate,
                    reorder: *reorder,
                    ..Default::default()
                })
                .await
        }
        FaultEvent::Heal => cluster.heal().await,
    }
    Ok(event.label())
}

/// Run the workload and the schedule together, and hand back the recorded history.
pub async fn run(
    cluster: &mut Cluster,
    spec: &WorkloadSpec,
    schedule: &FaultSchedule,
) -> Result<WorkloadResult, Failure> {
    let urls: Vec<String> = cluster
        .members
        .iter()
        .take(cluster.initial_size)
        .map(|m| format!("http://127.0.0.1:{}", m.client_port))
        .collect();
    let recorder = Arc::new(Recorder::new());
    let stop = Arc::new(AtomicBool::new(false));
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel::<(Entry, usize)>();
    let keys = spec.key_names();

    let mut tasks = Vec::new();
    for client_id in 0..spec.clients {
        let urls = urls.clone();
        let keys = keys.clone();
        let spec = spec.clone();
        let recorder = recorder.clone();
        let stop = stop.clone();
        let tx = tx.clone();
        tasks.push(tokio::spawn(async move {
            client_loop(client_id, urls, keys, spec, recorder, stop, tx).await;
        }));
    }
    drop(tx);

    let mut applied = Vec::new();
    let start = std::time::Instant::now();
    for (at, event) in schedule.sorted() {
        let wait = at.saturating_sub(start.elapsed());
        if !wait.is_zero() {
            tokio::time::sleep(wait).await;
        }
        if start.elapsed() >= spec.duration {
            break;
        }
        let label = apply(cluster, &event).await?;
        applied.push(format!("+{:>5} ms  {label}", start.elapsed().as_millis()));
    }
    let left = spec.duration.saturating_sub(start.elapsed());
    if !left.is_zero() {
        tokio::time::sleep(left).await;
    }
    stop.store(true, Ordering::Relaxed);
    for t in tasks {
        let _ = t.await;
    }

    let mut history = History::new();
    let mut per_member = vec![0usize; urls.len()];
    let mut acknowledged = 0;
    let mut unknown = 0;
    let mut collected: Vec<(Entry, usize)> = Vec::new();
    while let Ok(item) = rx.try_recv() {
        collected.push(item);
    }
    collected.sort_by_key(|(e, _)| e.call_ns);
    for (entry, member) in collected {
        if let Some(slot) = per_member.get_mut(member) {
            *slot += 1;
        }
        if entry.outcome == Outcome::Unknown {
            unknown += 1;
        } else {
            acknowledged += 1;
        }
        history.push(entry);
    }
    Ok(WorkloadResult {
        history,
        acknowledged,
        unknown,
        per_member,
        faults_applied: applied,
    })
}

#[allow(clippy::too_many_arguments)]
async fn client_loop(
    client_id: usize,
    urls: Vec<String>,
    keys: Vec<String>,
    spec: WorkloadSpec,
    recorder: Arc<Recorder>,
    stop: Arc<AtomicBool>,
    tx: tokio::sync::mpsc::UnboundedSender<(Entry, usize)>,
) {
    let mut rng = StdRng::seed_from_u64(spec.seed ^ ((client_id as u64 + 1) * 0x9e37_79b9));
    let mut clients: Vec<Client> = Vec::new();
    for (i, u) in urls.iter().enumerate() {
        match Client::new(u, &format!("m{}", i + 1), spec.op_timeout) {
            Ok(c) => clients.push(c),
            Err(_) => return,
        }
    }
    // Values this client has written, so a compare-and-swap has something plausible to
    // compare against.
    let mut known: Vec<Option<Vec<u8>>> = vec![None; keys.len()];
    let mut counter = 0u64;
    while !stop.load(Ordering::Relaxed) {
        let member = rng.random_range(0..clients.len());
        let ki = rng.random_range(0..keys.len());
        let key = keys[ki].clone();
        counter += 1;
        let choice = rng.random_range(0..100);
        let op = if choice < 40 {
            Op::Write(format!("c{client_id}-{counter}").into_bytes())
        } else if choice < 75 {
            Op::Read
        } else if choice < 90 {
            Op::Cas {
                expect: known[ki].clone(),
                new: format!("c{client_id}-cas{counter}").into_bytes(),
            }
        } else {
            Op::Delete
        };
        let call = recorder.now();
        let outcome = execute(&clients[member], key.as_bytes(), &op, spec.op_timeout).await;
        let ret = recorder.now();
        match (&op, &outcome) {
            (Op::Write(v), Outcome::Ok) => known[ki] = Some(v.clone()),
            (Op::Cas { new, .. }, Outcome::Swapped(true)) => known[ki] = Some(new.clone()),
            (Op::Delete, Outcome::Ok) => known[ki] = None,
            (Op::Read, Outcome::Value(v)) => known[ki] = v.clone(),
            _ => {}
        }
        let entry = recorder.entry(
            client_id,
            &key,
            op,
            outcome,
            call,
            ret,
            format!("m{}", member + 1),
        );
        if tx.send((entry, member)).is_err() {
            return;
        }
        if !spec.pause.is_zero() {
            tokio::time::sleep(spec.pause).await;
        }
    }
}

/// Run one operation and turn whatever happened into an [`Outcome`].
///
/// Every error becomes [`Outcome::Unknown`]: the harness genuinely cannot tell a request
/// that was refused from one that was applied and whose answer was lost, and only the
/// pessimistic reading is sound.
pub async fn execute(client: &Client, key: &[u8], op: &Op, timeout: Duration) -> Outcome {
    let fut = async {
        match op {
            Op::Read => match client.get_key(key).await {
                Ok(r) => Outcome::Value(r.one().map(|kv| kv.value.clone())),
                Err(_) => Outcome::Unknown,
            },
            Op::Write(v) => match client.put(key, v).await {
                Ok(_) => Outcome::Ok,
                Err(_) => Outcome::Unknown,
            },
            Op::Delete => match client.delete(key).await {
                Ok(_) => Outcome::Ok,
                Err(_) => Outcome::Unknown,
            },
            Op::Cas { expect, new } => {
                let compare = match expect {
                    Some(v) => etcd::cmp_value_eq(key, v),
                    None => etcd::cmp_create_eq(key, 0),
                };
                match client
                    .txn(vec![compare], vec![etcd::op_put(key, new)], vec![])
                    .await
                {
                    Ok(r) => Outcome::Swapped(r.succeeded),
                    Err(_) => Outcome::Unknown,
                }
            }
        }
    };
    tokio::time::timeout(timeout, fut)
        .await
        .unwrap_or(Outcome::Unknown)
}

/// Read every key from every running member and complain when they disagree.
pub async fn verify_convergence(
    cluster: &mut Cluster,
    keys: &[String],
    within: Duration,
) -> Result<Vec<(String, Option<String>)>, Failure> {
    let deadline = std::time::Instant::now() + within;
    loop {
        let running = cluster.running();
        let mut per_member: Vec<Vec<Option<String>>> = Vec::new();
        let mut failed = None;
        for i in &running {
            let mut row = Vec::new();
            for k in keys {
                match cluster.members[*i].client.value_of(k.as_bytes()).await {
                    Ok(v) => row.push(v.map(|b| String::from_utf8_lossy(&b).to_string())),
                    Err(e) => {
                        failed = Some(format!("{}: {e}", cluster.members[*i].name));
                        row.push(None);
                    }
                }
            }
            per_member.push(row);
        }
        let agreed = failed.is_none()
            && per_member
                .windows(2)
                .all(|w| w.first() == w.get(1))
            && !per_member.is_empty();
        if agreed {
            return Ok(keys
                .iter()
                .cloned()
                .zip(per_member[0].clone())
                .collect());
        }
        if std::time::Instant::now() >= deadline {
            let mut f = Failure::new(
                crate::assert::FailureKind::Assertion,
                format!(
                    "the members never agreed on the {} key(s) within {} ms",
                    keys.len(),
                    within.as_millis()
                ),
            );
            for (n, row) in running.iter().zip(per_member.iter()) {
                f.notes
                    .push(format!("m{} sees {:?}", n + 1, row));
            }
            if let Some(e) = failed {
                f.notes.push(e);
            }
            return Err(f);
        }
        tokio::time::sleep(Duration::from_millis(150)).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seeded_schedule_is_reproducible_and_always_heals() {
        let a = FaultSchedule::random(42, 3, Duration::from_millis(6_000));
        let b = FaultSchedule::random(42, 3, Duration::from_millis(6_000));
        assert_eq!(a.events, b.events);
        let c = FaultSchedule::random(43, 3, Duration::from_millis(6_000));
        assert_ne!(a.events, c.events, "a different seed plans differently");
        let sorted = a.sorted();
        assert!(sorted.len() >= 6);
        let last_two: Vec<&FaultEvent> = sorted.iter().rev().take(2).map(|(_, e)| e).collect();
        assert!(
            last_two.contains(&&FaultEvent::RestartStopped) && last_two.contains(&&FaultEvent::Heal),
            "a schedule must end healed, got {:?}",
            sorted.iter().map(|(_, e)| e.label()).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_partition_event_always_leaves_a_majority() {
        for seed in 0..40u64 {
            for (_, e) in FaultSchedule::random(seed, 5, Duration::from_millis(6_000)).events {
                if let FaultEvent::Partition(groups) = e {
                    let sizes: Vec<usize> = groups.iter().map(Vec::len).collect();
                    assert_eq!(sizes.iter().sum::<usize>(), 5, "every member is placed once");
                    assert!(
                        sizes.iter().any(|s| *s >= 3),
                        "one side must keep a quorum, got {sizes:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn events_render_readably() {
        assert_eq!(FaultEvent::Isolate(1).label(), "isolate m2");
        assert_eq!(
            FaultEvent::Partition(vec![vec![0], vec![1, 2]]).label(),
            "partition {m1} | {m2,m3}"
        );
        assert_eq!(
            FaultEvent::DelayAll(Duration::from_millis(50)).label(),
            "delay every link by 50 ms"
        );
        assert_eq!(FaultSchedule::none().render(), "no faults");
        assert!(FaultSchedule::random(1, 3, Duration::from_millis(3000))
            .render()
            .contains("ms"));
    }

    #[test]
    fn a_workload_spec_names_its_own_keys() {
        let s = WorkloadSpec::small(1, "s54");
        assert_eq!(s.key_names(), vec!["s54/k0", "s54/k1", "s54/k2"]);
    }
}
