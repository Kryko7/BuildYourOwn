//! Spawning, watching and killing the broker under test.
//!
//! The child is put in its own process group so that a shell wrapper
//! (`./your_program.sh` → `java` → ...) is killed as a tree and no JVM is ever leaked.

pub mod reference;

use crate::assert::{Failure, FailureKind};
use crate::config::{BrokerDef, BrokerKind};
use anyhow::{Context, Result};
use std::net::{SocketAddr, TcpListener};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Everything needed to start one broker process.
#[derive(Debug, Clone)]
pub struct BrokerSpec {
    /// Broker name, for messages.
    pub name: String,
    /// Full argv, placeholders already substituted.
    pub argv: Vec<String>,
    /// Working directory of the child.
    pub cwd: PathBuf,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
    /// The port the broker must listen on.
    pub port: u16,
    /// Kafka log directory (fixtures are written here).
    pub log_dir: PathBuf,
    /// The properties file passed as argv[1].
    pub props: PathBuf,
    /// Scratch directory for this broker instance.
    pub tmp: PathBuf,
    /// How long to wait for the port to accept connections.
    pub boot_timeout: Duration,
}

/// A running (or crashed) broker process.
pub struct BrokerHandle {
    /// The spec it was started from.
    pub spec: BrokerSpec,
    /// Where clients should connect.
    pub addr: SocketAddr,
    child: Option<Child>,
    pgid: i32,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    /// How long the broker took to accept its first connection.
    pub boot_ms: u128,
}

impl BrokerHandle {
    /// Start the process and wait until its port accepts connections.
    pub fn start(spec: BrokerSpec) -> Result<BrokerHandle> {
        std::fs::create_dir_all(&spec.tmp)
            .with_context(|| format!("cannot create {}", spec.tmp.display()))?;
        std::fs::create_dir_all(&spec.log_dir)
            .with_context(|| format!("cannot create {}", spec.log_dir.display()))?;
        let stdout_path = spec.tmp.join("broker.stdout");
        let stderr_path = spec.tmp.join("broker.stderr");
        let (program, args) = spec.argv.split_first().context("broker command is empty")?;
        let out = std::fs::File::create(&stdout_path)?;
        let err = std::fs::File::create(&stderr_path)?;
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
        let child = cmd
            .spawn()
            .with_context(|| format!("cannot start broker '{}': {program}", spec.name))?;
        let pgid = child.id() as i32;
        let addr: SocketAddr = format!("127.0.0.1:{}", spec.port)
            .parse()
            .context("broker address")?;
        let mut handle = BrokerHandle {
            spec,
            addr,
            child: Some(child),
            pgid,
            stdout_path,
            stderr_path,
            boot_ms: 0,
        };
        let started = Instant::now();
        handle.wait_for_port()?;
        handle.boot_ms = started.elapsed().as_millis();
        Ok(handle)
    }

    fn wait_for_port(&mut self) -> Result<()> {
        let deadline = Instant::now() + self.spec.boot_timeout;
        loop {
            if let Some(status) = self.exited() {
                anyhow::bail!(
                    "broker '{}' exited with {} before it listened on port {}\n{}",
                    self.spec.name,
                    describe_status(&status),
                    self.spec.port,
                    self.output_tail(30)
                );
            }
            if std::net::TcpStream::connect_timeout(&self.addr, Duration::from_millis(200)).is_ok()
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "broker '{}' did not listen on port {} within {} ms\n{}",
                    self.spec.name,
                    self.spec.port,
                    self.spec.boot_timeout.as_millis(),
                    self.output_tail(30)
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    /// `Some(status)` once the process has exited.
    pub fn exited(&mut self) -> Option<std::process::ExitStatus> {
        match self.child.as_mut() {
            Some(c) => c.try_wait().ok().flatten(),
            None => None,
        }
    }

    /// A failure describing a broker that died mid-test, or `None` while it is alive.
    pub fn crash_failure(&mut self) -> Option<Failure> {
        let status = self.exited()?;
        let tail = self.output_tail(30);
        let mut f = Failure::new(
            FailureKind::BrokerCrash,
            format!(
                "broker '{}' exited with {} during the test",
                self.spec.name,
                describe_status(&status)
            ),
        );
        f.notes.push(tail);
        Some(f)
    }

    /// The last `lines` lines of the broker's stdout and stderr.
    pub fn output_tail(&self, lines: usize) -> String {
        let mut out = String::new();
        for (label, path) in [
            ("broker stdout", &self.stdout_path),
            ("broker stderr", &self.stderr_path),
        ] {
            let text = std::fs::read_to_string(path).unwrap_or_default();
            let trimmed = text.trim_end();
            if trimmed.is_empty() {
                continue;
            }
            let tail: Vec<&str> = trimmed
                .lines()
                // Kafka logs its whole configuration at INFO; those continuation lines
                // start with a tab and would bury the interesting output.
                .filter(|l| !l.starts_with('\t') && !l.trim_start().starts_with("(org.apache"))
                .rev()
                .take(lines)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            out.push_str(&format!("{label} (last {} lines):\n", tail.len()));
            for l in tail {
                out.push_str("  ");
                out.push_str(l);
                out.push('\n');
            }
        }
        if out.is_empty() {
            out.push_str("broker produced no output\n");
        }
        out.trim_end().to_string()
    }

    /// Kill the whole process group and reap the child.
    pub fn stop(&mut self) {
        let Some(mut child) = self.child.take() else {
            return;
        };
        // SAFETY: killpg on our own child's process group; the pgid was set at spawn time.
        unsafe {
            libc::killpg(self.pgid, libc::SIGTERM);
        }
        let deadline = Instant::now() + Duration::from_millis(2000);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break,
                Ok(None) if Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(20));
                }
                _ => {
                    // SAFETY: same process group, now with SIGKILL.
                    unsafe {
                        libc::killpg(self.pgid, libc::SIGKILL);
                    }
                    let _ = child.wait();
                    break;
                }
            }
        }
        // Anything else left in the group (a wrapper's grandchildren) goes too.
        // SAFETY: the group is ours and the leader has already been reaped.
        unsafe {
            libc::killpg(self.pgid, libc::SIGKILL);
        }
    }
}

impl Drop for BrokerHandle {
    fn drop(&mut self) {
        self.stop();
    }
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
/// Never returns 9092: other agents may be running their own brokers on this machine.
pub fn free_port() -> Result<u16> {
    for _ in 0..64 {
        let listener = TcpListener::bind("127.0.0.1:0").context("cannot bind a free port")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        if port != 9092 {
            return Ok(port);
        }
    }
    anyhow::bail!("could not find a free port")
}

/// The `server.properties` handed to a non-reference broker as argv[1].
///
/// It is deliberately close to what the challenge writes: a log directory and
/// a plaintext listener, nothing else a hand-written broker would have to parse.
pub fn write_external_properties(path: &Path, log_dir: &Path, port: u16) -> Result<()> {
    let text = format!(
        "# written by kafkatest\n\
         broker.id=1\n\
         node.id=1\n\
         process.roles=broker,controller\n\
         listeners=PLAINTEXT://0.0.0.0:{port},CONTROLLER://0.0.0.0:{controller}\n\
         advertised.listeners=PLAINTEXT://127.0.0.1:{port}\n\
         listener.security.protocol.map=CONTROLLER:PLAINTEXT,PLAINTEXT:PLAINTEXT\n\
         controller.listener.names=CONTROLLER\n\
         inter.broker.listener.name=PLAINTEXT\n\
         log.dirs={log}\n\
         num.partitions=1\n\
         auto.create.topics.enable=false\n\
         message.max.bytes={max_message}\n",
        port = port,
        controller = port.wrapping_add(1000).max(1024),
        log = log_dir.display(),
        max_message = reference::MESSAGE_MAX_BYTES,
    );
    std::fs::write(path, text).with_context(|| format!("cannot write {}", path.display()))?;
    Ok(())
}

/// Build the spec for a non-reference broker.
pub fn external_spec(def: &BrokerDef, tmp: &Path, port: u16) -> Result<BrokerSpec> {
    debug_assert_eq!(def.kind, BrokerKind::External);
    let log_dir = match &def.log_dir {
        Some(d) => PathBuf::from(d),
        None => tmp.join("kraft-combined-logs"),
    };
    let props = tmp.join("server.properties");
    let ph = crate::config::Placeholders {
        props: props.clone(),
        log_dir: log_dir.clone(),
        tmp: tmp.to_path_buf(),
        port,
    };
    let log_dir = PathBuf::from(ph.apply(&log_dir.to_string_lossy()));
    let ph = crate::config::Placeholders {
        log_dir: log_dir.clone(),
        ..ph
    };
    let argv: Vec<String> = def.argv_or_default().iter().map(|a| ph.apply(a)).collect();
    let cwd = def
        .cwd
        .as_ref()
        .map(|c| PathBuf::from(ph.apply(c)))
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let env = def
        .env
        .iter()
        .map(|(k, v)| (k.clone(), ph.apply(v)))
        .collect();
    Ok(BrokerSpec {
        name: def.name.clone(),
        argv,
        cwd,
        env,
        port,
        log_dir,
        props,
        tmp: tmp.to_path_buf(),
        boot_timeout: Duration::from_millis(def.boot_timeout_ms),
    })
}

impl BrokerDef {
    /// The configured argv (reference brokers fill this in themselves).
    pub fn argv_or_default(&self) -> &[String] {
        &self.command
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn free_port_is_usable_and_never_9092() {
        let p = free_port().expect("free port");
        assert_ne!(p, 9092);
        let l = TcpListener::bind(("127.0.0.1", p)).expect("bind the port we were given");
        drop(l);
    }

    #[test]
    fn external_properties_mention_the_log_dir_and_port() {
        let dir = tempfile::tempdir().expect("tempdir");
        let p = dir.path().join("server.properties");
        write_external_properties(&p, &dir.path().join("logs"), 12345).expect("write");
        let text = std::fs::read_to_string(&p).expect("read");
        assert!(
            text.contains("listeners=PLAINTEXT://0.0.0.0:12345"),
            "{text}"
        );
        assert!(text.contains(&format!("log.dirs={}", dir.path().join("logs").display())));
        assert!(
            text.contains(&format!(
                "message.max.bytes={}",
                reference::MESSAGE_MAX_BYTES
            )),
            "stage 37 needs the size limit in the properties file\n{text}"
        );
    }

    #[test]
    fn a_broker_that_exits_immediately_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let spec = BrokerSpec {
            name: "false".into(),
            argv: vec!["/bin/false".into()],
            cwd: dir.path().to_path_buf(),
            env: vec![],
            port: free_port().expect("port"),
            log_dir: dir.path().join("logs"),
            props: dir.path().join("server.properties"),
            tmp: dir.path().to_path_buf(),
            boot_timeout: Duration::from_millis(2000),
        };
        let err = match BrokerHandle::start(spec) {
            Ok(_) => panic!("/bin/false must not look like a running broker"),
            Err(e) => e,
        };
        assert!(format!("{err}").contains("exited with status 1"), "{err}");
    }

    #[test]
    fn a_listening_broker_boots_and_dies_with_its_group() {
        let dir = tempfile::tempdir().expect("tempdir");
        let port = free_port().expect("port");
        // A shell wrapper around a listener: proves the whole process group is killed.
        let script = dir.path().join("listen.sh");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nexec nc -l 127.0.0.1 {port} >/dev/null 2>&1\n"),
        )
        .expect("write script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
                .expect("chmod");
        }
        if which("nc").is_none() {
            return; // nc is not installed; nothing to prove here
        }
        let spec = BrokerSpec {
            name: "nc".into(),
            argv: vec![script.to_string_lossy().to_string()],
            cwd: dir.path().to_path_buf(),
            env: vec![],
            port,
            log_dir: dir.path().join("logs"),
            props: dir.path().join("server.properties"),
            tmp: dir.path().to_path_buf(),
            boot_timeout: Duration::from_millis(5000),
        };
        let Ok(mut h) = BrokerHandle::start(spec) else {
            return; // nc flavours differ; do not fail the suite over it
        };
        assert!(h.exited().is_none());
        h.stop();
        std::thread::sleep(Duration::from_millis(100));
        assert!(
            std::net::TcpStream::connect_timeout(&h.addr, Duration::from_millis(200)).is_err(),
            "the port is still open after stop()"
        );
    }

    fn which(prog: &str) -> Option<PathBuf> {
        std::env::split_paths(&std::env::var_os("PATH")?)
            .map(|d| d.join(prog))
            .find(|p| p.is_file())
    }
}
