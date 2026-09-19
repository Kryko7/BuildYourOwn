//! Spawning, watching and killing one node of the system under test.
//!
//! The child is put in its own process group, so a shell wrapper
//! (`./your_program.sh` → a real binary → ...) dies as a tree and no server is ever leaked
//! into a later stage. Every path out of a test — success, assertion, panic, timeout —
//! goes through [`NodeHandle::stop`], because [`Drop`] calls it too.

pub mod reference;

use crate::assert::{Failure, FailureKind};
use crate::config::{Placeholders, TargetDef, TargetKind};
use anyhow::{bail, Context, Result};
use std::net::{SocketAddr, TcpListener};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Everything needed to start one node process.
#[derive(Debug, Clone)]
pub struct NodeSpec {
    /// The target's name, for messages.
    pub target: String,
    /// The member name (`m1`, `m2`, ...).
    pub name: String,
    /// Full argv, placeholders already substituted.
    pub argv: Vec<String>,
    /// Working directory of the child.
    pub cwd: PathBuf,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
    /// The client port this node must listen on.
    pub client_port: u16,
    /// The peer port this node must listen on.
    pub peer_port: u16,
    /// The data directory; a restart must find everything it needs here.
    pub data_dir: PathBuf,
    /// Scratch directory for this node instance (process output lands here).
    pub tmp: PathBuf,
    /// How long to wait for the client URL to answer.
    pub boot_timeout: Duration,
}

impl NodeSpec {
    /// `http://127.0.0.1:<client port>`, the base URL every request is sent to.
    pub fn client_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.client_port)
    }

    /// `http://127.0.0.1:<peer port>`, what this node listens on for peer traffic.
    pub fn peer_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.peer_port)
    }
}

/// A running (or crashed) node process.
pub struct NodeHandle {
    /// The spec it was started from.
    pub spec: NodeSpec,
    /// Where clients connect.
    pub addr: SocketAddr,
    child: Option<Child>,
    pgid: i32,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    /// How long the node took to answer its first request.
    pub boot_ms: u128,
}

impl NodeHandle {
    /// Start the process, without waiting for it to be ready.
    pub fn spawn(spec: NodeSpec) -> Result<NodeHandle> {
        std::fs::create_dir_all(&spec.tmp)
            .with_context(|| format!("cannot create {}", spec.tmp.display()))?;
        std::fs::create_dir_all(&spec.data_dir)
            .with_context(|| format!("cannot create {}", spec.data_dir.display()))?;
        let stdout_path = spec.tmp.join(format!("{}.stdout", spec.name));
        let stderr_path = spec.tmp.join(format!("{}.stderr", spec.name));
        let (program, args) = spec.argv.split_first().context("node command is empty")?;
        // Appending keeps the output of a node that is stopped and started in place, which
        // is exactly the case a durability test wants to read afterwards.
        let out = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stdout_path)?;
        let err = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&stderr_path)?;
        let mut cmd = Command::new(program);
        cmd.args(args)
            .current_dir(&spec.cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err))
            .process_group(0);
        for (k, v) in &spec.env {
            cmd.env(k, v);
        }
        let child = spawn_retrying_on_etxtbsy(&mut cmd)
            .with_context(|| format!("cannot start node '{}': {program}", spec.name))?;
        let pgid = child.id() as i32;
        let addr: SocketAddr = format!("127.0.0.1:{}", spec.client_port)
            .parse()
            .context("node address")?;
        Ok(NodeHandle {
            spec,
            addr,
            child: Some(child),
            pgid,
            stdout_path,
            stderr_path,
            boot_ms: 0,
        })
    }

    /// Start the process and wait until its client URL answers `GET /health`.
    pub fn start(spec: NodeSpec) -> Result<NodeHandle> {
        let mut handle = NodeHandle::spawn(spec)?;
        let started = Instant::now();
        handle.wait_until_ready()?;
        handle.boot_ms = started.elapsed().as_millis();
        Ok(handle)
    }

    /// Block until the node answers on its client URL, or fail with its own output.
    pub fn wait_until_ready(&mut self) -> Result<()> {
        let deadline = Instant::now() + self.spec.boot_timeout;
        let mut last;
        loop {
            if let Some(status) = self.exited() {
                bail!(
                    "node '{}' exited with {} before it answered on port {}\n{}",
                    self.spec.name,
                    describe_status(&status),
                    self.spec.client_port,
                    self.output_tail(30)
                );
            }
            match crate::etcd::blocking_probe(&self.spec.client_url()) {
                Ok(()) => return Ok(()),
                Err(e) => last = e,
            }
            if Instant::now() >= deadline {
                bail!(
                    "node '{}' did not answer GET /health on port {} within {} ms ({last})\n{}",
                    self.spec.name,
                    self.spec.client_port,
                    self.spec.boot_timeout.as_millis(),
                    self.output_tail(30)
                );
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    /// The child's pid, while it is running.
    ///
    /// The fault injector needs it: a peer connection is traced back to the member that
    /// opened it through `/proc/<pid>/fd`.
    pub fn pid(&self) -> Option<u32> {
        self.child.as_ref().map(Child::id)
    }

    /// `Some(status)` once the process has exited.
    pub fn exited(&mut self) -> Option<std::process::ExitStatus> {
        match self.child.as_mut() {
            Some(c) => c.try_wait().ok().flatten(),
            None => None,
        }
    }

    /// True while the process is still running.
    pub fn alive(&mut self) -> bool {
        self.child.is_some() && self.exited().is_none()
    }

    /// A failure describing a node that died mid-test, or `None` while it is alive.
    pub fn crash_failure(&mut self) -> Option<Failure> {
        let status = self.exited()?;
        let tail = self.output_tail(30);
        Some(
            Failure::new(
                FailureKind::NodeCrash,
                format!(
                    "node '{}' exited with {} during the test",
                    self.spec.name,
                    describe_status(&status)
                ),
            )
            .note(tail),
        )
    }

    /// The last `lines` lines of the node's stdout and stderr.
    pub fn output_tail(&self, lines: usize) -> String {
        let mut out = String::new();
        for (label, path) in [
            ("node stdout", &self.stdout_path),
            ("node stderr", &self.stderr_path),
        ] {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let trimmed = text.trim_end();
            if trimmed.is_empty() {
                continue;
            }
            let tail: Vec<&str> = trimmed
                .lines()
                .rev()
                .take(lines)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            out.push_str(&format!(
                "{label} of {} (last {} lines):\n",
                self.spec.name,
                tail.len()
            ));
            for l in tail {
                out.push_str("  ");
                out.push_str(&truncate(l, 300));
                out.push('\n');
            }
        }
        if out.is_empty() {
            out.push_str(&format!("node {} produced no output\n", self.spec.name));
        }
        out.trim_end().to_string()
    }

    /// Kill the process group with `SIGKILL` and reap it: a power cut, not a shutdown.
    ///
    /// This is what every durability stage uses. Nothing is flushed, no handler runs, and
    /// whatever survives is what the node had already written to its data directory.
    pub fn kill_hard(&mut self) {
        self.signal_and_reap(libc::SIGKILL);
    }

    /// Stop the process group politely (`SIGTERM`, then `SIGKILL`) and reap it.
    pub fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            // The group may still hold grandchildren even when the leader is already reaped.
            self.killpg(libc::SIGKILL);
            return;
        };
        self.killpg(libc::SIGTERM);
        let deadline = Instant::now() + Duration::from_millis(2000);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    self.killpg(libc::SIGKILL);
                    let _ = child.wait();
                    break;
                }
            }
        }
        self.killpg(libc::SIGKILL);
    }

    fn signal_and_reap(&mut self, sig: i32) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        self.killpg(sig);
        let _ = child.wait();
        self.killpg(libc::SIGKILL);
    }

    fn killpg(&self, sig: i32) {
        if self.pgid <= 1 {
            return;
        }
        // SAFETY: killpg on a process group this handle created at spawn time.
        unsafe {
            libc::killpg(self.pgid, sig);
        }
    }

    /// Start the same spec again: same ports, same data directory, nothing reformatted.
    ///
    /// What is on disk is all the new process has, which is the whole point of the
    /// durability and whole-cluster-restart stages.
    pub fn restart_in_place(&mut self) -> Result<()> {
        self.stop();
        // Adopt the fresh process's child and group, and make sure the temporary handle's
        // `Drop` cannot kill what this handle has just taken over.
        let mut fresh = std::mem::ManuallyDrop::new(NodeHandle::spawn(self.spec.clone())?);
        self.child = fresh.child.take();
        self.pgid = fresh.pgid;
        let started = Instant::now();
        self.wait_until_ready()?;
        self.boot_ms = started.elapsed().as_millis();
        Ok(())
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let head: String = s.chars().take(max).collect();
    format!("{head}… ({} chars)", s.chars().count())
}

impl Drop for NodeHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Spawn, retrying briefly while the kernel says the program is still open for writing.
///
/// `ETXTBSY` means some process still holds a writable handle on the file being executed,
/// which happens for a few milliseconds after anything writes a wrapper script. It is a
/// race, not a broken program, and failing on it would blame the learner for the harness.
fn spawn_retrying_on_etxtbsy(cmd: &mut Command) -> std::io::Result<Child> {
    let mut last = None;
    for attempt in 0..10 {
        match cmd.spawn() {
            Ok(child) => return Ok(child),
            Err(e) if e.raw_os_error() == Some(libc::ETXTBSY) => {
                std::thread::sleep(Duration::from_millis(10 * (attempt + 1)));
                last = Some(e);
            }
            Err(e) => return Err(e),
        }
    }
    Err(last.unwrap_or_else(|| std::io::Error::other("the node could not be started")))
}

/// Human-readable exit status ("status 1", "signal 9").
pub fn describe_status(status: &std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match (status.code(), status.signal()) {
        (Some(c), _) => format!("status {c}"),
        (None, Some(s)) => format!("signal {s}"),
        _ => "an unknown status".to_string(),
    }
}

/// Ask the OS for a free TCP port by binding to port 0 and closing again.
///
/// Nothing in this suite ever binds a fixed port: several agents, several stages and the
/// learner's own server may be running on this machine at the same time.
pub fn free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("cannot bind a free port")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// `n` free ports at once, all different.
pub fn free_ports(n: usize) -> Result<Vec<u16>> {
    let mut held = Vec::new();
    let mut ports = Vec::new();
    for _ in 0..n {
        let l = TcpListener::bind("127.0.0.1:0").context("cannot bind a free port")?;
        ports.push(l.local_addr()?.port());
        held.push(l);
    }
    drop(held);
    Ok(ports)
}

/// The argv a node of the `node`/`cluster` ladders is started with.
///
/// Exactly the etcd flag subset the README documents, in a fixed order, so a learner can
/// parse it with a two-line loop.
pub fn node_argv(
    base: &[String],
    name: &str,
    data_dir: &Path,
    client_port: u16,
    peer_port: u16,
    initial_cluster: &str,
    advertise_peer: &str,
) -> Vec<String> {
    let mut argv: Vec<String> = base.to_vec();
    let mut push = |k: &str, v: String| {
        argv.push(k.to_string());
        argv.push(v);
    };
    push("--name", name.to_string());
    push("--data-dir", data_dir.to_string_lossy().to_string());
    push(
        "--listen-client-urls",
        format!("http://127.0.0.1:{client_port}"),
    );
    push(
        "--advertise-client-urls",
        format!("http://127.0.0.1:{client_port}"),
    );
    push(
        "--listen-peer-urls",
        format!("http://127.0.0.1:{peer_port}"),
    );
    push("--initial-advertise-peer-urls", advertise_peer.to_string());
    push("--initial-cluster", initial_cluster.to_string());
    argv
}

/// Build a [`NodeSpec`] for one member of a target.
#[allow(clippy::too_many_arguments)]
pub fn spec_for(
    def: &TargetDef,
    dist: Option<&Path>,
    name: &str,
    tmp: &Path,
    data_dir: &Path,
    client_port: u16,
    peer_port: u16,
    initial_cluster: &str,
    advertise_peer: &str,
) -> Result<NodeSpec> {
    let ph = Placeholders {
        data_dir: data_dir.to_path_buf(),
        tmp: tmp.to_path_buf(),
        port: client_port,
        peer_port,
        node: name.to_string(),
    };
    let base: Vec<String> = match def.kind {
        TargetKind::Reference => {
            let dist = dist.context("the reference distribution was never unpacked")?;
            vec![dist.join("etcd").to_string_lossy().to_string()]
        }
        TargetKind::Example => vec![crate::node::reference::example_binary(&def.name)?
            .to_string_lossy()
            .to_string()],
        TargetKind::External => def.command.iter().map(|a| ph.apply(a)).collect(),
    };
    let mut env: Vec<(String, String)> = def
        .env
        .iter()
        .map(|(k, v)| (k.clone(), ph.apply(v)))
        .collect();
    if def.kind == TargetKind::Reference {
        // Quiet, predictable logs; the harness reads readiness over HTTP, never from stdout.
        env.push(("ETCD_LOG_LEVEL".into(), "warn".into()));
    }
    let cwd = def
        .cwd
        .as_ref()
        .map(|c| PathBuf::from(ph.apply(c)))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    Ok(NodeSpec {
        target: def.name.clone(),
        name: name.to_string(),
        argv: node_argv(
            &base,
            name,
            data_dir,
            client_port,
            peer_port,
            initial_cluster,
            advertise_peer,
        ),
        cwd,
        env,
        client_port,
        peer_port,
        data_dir: data_dir.to_path_buf(),
        tmp: tmp.to_path_buf(),
        boot_timeout: Duration::from_millis(def.boot_timeout_ms),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_ports_are_distinct_and_usable() {
        // free_ports() binds to learn each number and lets go again, so anything on the
        // machine may take one before this test re-binds it. The properties under test are
        // distinctness and bindability, not that the kernel reserves them for us, so a lost
        // race is retried rather than failed.
        for attempt in 0..8 {
            let ports = free_ports(4).expect("ports");
            let mut sorted = ports.clone();
            sorted.sort_unstable();
            sorted.dedup();
            assert_eq!(sorted.len(), 4, "four distinct ports");
            let bound: Vec<_> = ports
                .iter()
                .map(|p| TcpListener::bind(("127.0.0.1", *p)))
                .collect();
            if bound.iter().all(Result::is_ok) {
                return;
            }
            assert!(
                attempt < 7,
                "free_ports never handed back four bindable ports"
            );
        }
    }

    #[test]
    fn node_argv_is_the_documented_flag_subset() {
        let argv = node_argv(
            &["./your_program.sh".to_string()],
            "m1",
            Path::new("/tmp/d"),
            2379,
            2380,
            "m1=http://127.0.0.1:2380",
            "http://127.0.0.1:2380",
        );
        assert_eq!(argv[0], "./your_program.sh");
        let joined = argv.join(" ");
        for flag in [
            "--name m1",
            "--data-dir /tmp/d",
            "--listen-client-urls http://127.0.0.1:2379",
            "--advertise-client-urls http://127.0.0.1:2379",
            "--listen-peer-urls http://127.0.0.1:2380",
            "--initial-advertise-peer-urls http://127.0.0.1:2380",
            "--initial-cluster m1=http://127.0.0.1:2380",
        ] {
            assert!(joined.contains(flag), "{flag} missing from: {joined}");
        }
    }

    #[test]
    fn a_node_that_exits_immediately_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spec = NodeSpec {
            target: "false".into(),
            name: "m1".into(),
            argv: vec!["/bin/false".into()],
            cwd: dir.path().to_path_buf(),
            env: vec![],
            client_port: free_port().expect("port"),
            peer_port: free_port().expect("port"),
            data_dir: dir.path().join("data"),
            tmp: dir.path().to_path_buf(),
            boot_timeout: Duration::from_millis(2000),
        };
        let err = match NodeHandle::start(spec) {
            Ok(_) => panic!("/bin/false must not look like a running node"),
            Err(e) => e,
        };
        assert!(format!("{err}").contains("exited with status 1"), "{err}");
    }

    #[test]
    fn stopping_a_wrapper_kills_the_whole_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let port = free_port().expect("port");
        let script = dir.path().join("wrap.sh");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nexec python3 -c \"import http.server,socketserver;\
                 socketserver.TCPServer(('127.0.0.1',{port}),http.server.SimpleHTTPRequestHandler)\
                 .serve_forever()\"\n"
            ),
        )
        .expect("write");
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        let spec = NodeSpec {
            target: "wrapper".into(),
            name: "m1".into(),
            argv: vec![script.to_string_lossy().to_string()],
            cwd: dir.path().to_path_buf(),
            env: vec![],
            client_port: port,
            peer_port: free_port().expect("port"),
            data_dir: dir.path().join("data"),
            tmp: dir.path().to_path_buf(),
            boot_timeout: Duration::from_millis(5000),
        };
        let mut h = NodeHandle::spawn(spec).expect("spawn");
        // Wait for the grandchild to bind, without insisting it speaks our protocol.
        let deadline = Instant::now() + Duration::from_millis(5000);
        while Instant::now() < deadline
            && std::net::TcpStream::connect_timeout(&h.addr, Duration::from_millis(100)).is_err()
        {
            std::thread::sleep(Duration::from_millis(25));
        }
        h.stop();
        std::thread::sleep(Duration::from_millis(150));
        assert!(
            std::net::TcpStream::connect_timeout(&h.addr, Duration::from_millis(200)).is_err(),
            "the port is still open after stop(): a grandchild was leaked"
        );
    }
}
