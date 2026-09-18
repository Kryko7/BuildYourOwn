//! Spawning, watching and killing the server under test.
//!
//! The child goes into its own process group so that `./your_program.sh` → whatever it
//! execs is killed as a tree, and nothing is ever left holding a port. The suite never
//! binds a fixed port: every server instance gets a free one from the kernel, because
//! other agents run their own servers on this machine.

pub mod reference;

use crate::assert::{Failure, FailureKind};
use crate::certs::Material;
use crate::config::{Placeholders, ServerDef, ServerKind, ServerOptions};
use anyhow::{Context, Result};
use std::net::{SocketAddr, TcpListener};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// Everything needed to start one server process.
#[derive(Debug, Clone)]
pub struct ServerSpec {
    /// Server name, for messages.
    pub name: String,
    /// Full argv, the contract's flags included.
    pub argv: Vec<String>,
    /// Working directory of the child.
    pub cwd: PathBuf,
    /// Extra environment variables.
    pub env: Vec<(String, String)>,
    /// The port the server must listen on.
    pub port: u16,
    /// The certificate it was given.
    pub cert: PathBuf,
    /// The key it was given.
    pub key: PathBuf,
    /// Scratch directory for this server instance.
    pub tmp: PathBuf,
    /// How long to wait for the port to accept connections.
    pub boot_timeout: Duration,
    /// The options the runner started it with.
    pub options: ServerOptions,
}

impl ServerSpec {
    /// The command line, as it would be typed.
    pub fn command_line(&self) -> String {
        self.argv.join(" ")
    }
}

/// A running (or dead) server process.
pub struct ServerHandle {
    /// The spec it was started from.
    pub spec: ServerSpec,
    /// Where clients should connect.
    pub addr: SocketAddr,
    child: Option<Child>,
    pgid: i32,
    stdout_path: PathBuf,
    stderr_path: PathBuf,
    /// How long the server took to accept its first connection.
    pub boot_ms: u128,
}

impl ServerHandle {
    /// Start the process and wait until its port accepts connections.
    pub fn start(spec: ServerSpec) -> Result<ServerHandle> {
        std::fs::create_dir_all(&spec.tmp)
            .with_context(|| format!("cannot create {}", spec.tmp.display()))?;
        let stdout_path = spec.tmp.join("server.stdout");
        let stderr_path = spec.tmp.join("server.stderr");
        let (program, args) = spec.argv.split_first().context("server command is empty")?;
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
            .with_context(|| format!("cannot start server '{}': {program}", spec.name))?;
        let pgid = child.id() as i32;
        let addr: SocketAddr = format!("127.0.0.1:{}", spec.port)
            .parse()
            .context("server address")?;
        let mut handle = ServerHandle {
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
                    "server '{}' exited with {} before it listened on port {}\n{}\n{}",
                    self.spec.name,
                    describe_status(&status),
                    self.spec.port,
                    self.spec.command_line(),
                    self.output_tail(30)
                );
            }
            // A connect that succeeds also consumes one of the server's accepts, so the
            // probe socket is closed immediately and the caller reconnects. `-naccept n`
            // counts it, which is why the runner adds one to whatever a test asked for.
            if std::net::TcpStream::connect_timeout(&self.addr, Duration::from_millis(200)).is_ok()
            {
                return Ok(());
            }
            if Instant::now() >= deadline {
                anyhow::bail!(
                    "server '{}' did not listen on port {} within {} ms\n{}\n{}",
                    self.spec.name,
                    self.spec.port,
                    self.spec.boot_timeout.as_millis(),
                    self.spec.command_line(),
                    self.output_tail(30)
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// `Some(status)` once the process has exited.
    pub fn exited(&mut self) -> Option<std::process::ExitStatus> {
        match self.child.as_mut() {
            Some(c) => c.try_wait().ok().flatten(),
            None => None,
        }
    }

    /// A failure describing a server that died mid-test, or `None` while it is alive.
    pub fn crash_failure(&mut self) -> Option<Failure> {
        let status = self.exited()?;
        let tail = self.output_tail(30);
        let mut f = Failure::new(
            FailureKind::ServerCrash,
            format!(
                "server '{}' exited with {} during the test",
                self.spec.name,
                describe_status(&status)
            ),
        );
        f.notes.push(tail);
        Some(f)
    }

    /// The last `lines` lines of the server's stdout and stderr.
    pub fn output_tail(&self, lines: usize) -> String {
        let mut out = String::new();
        for (label, path) in [
            ("server stdout", &self.stdout_path),
            ("server stderr", &self.stderr_path),
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
            out.push_str(&format!("{label} (last {} lines):\n", tail.len()));
            for l in tail {
                out.push_str("  ");
                out.push_str(l);
                out.push('\n');
            }
        }
        if out.is_empty() {
            out.push_str("the server produced no output\n");
        }
        out.trim_end().to_string()
    }

    /// Wait for the process to exit, up to `timeout`.
    pub fn wait_for_exit(&mut self, timeout: Duration) -> Option<std::process::ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(s) = self.exited() {
                return Some(s);
            }
            if Instant::now() >= deadline {
                return None;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
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
                    std::thread::sleep(Duration::from_millis(10));
                }
                _ => {
                    // SAFETY: the same process group, now with SIGKILL.
                    unsafe {
                        libc::killpg(self.pgid, libc::SIGKILL);
                    }
                    let _ = child.wait();
                    break;
                }
            }
        }
        // Anything left in the group (a wrapper's grandchildren) goes too.
        // SAFETY: the group is ours and the leader has already been reaped.
        unsafe {
            libc::killpg(self.pgid, libc::SIGKILL);
        }
    }
}

impl Drop for ServerHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Human-readable exit status ("status 1", "signal 11").
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
/// Never returns 4433, `s_server`'s own default: another agent may be running one.
pub fn free_port() -> Result<u16> {
    for _ in 0..64 {
        let listener = TcpListener::bind("127.0.0.1:0").context("cannot bind a free port")?;
        let port = listener.local_addr()?.port();
        drop(listener);
        if port != 4433 {
            return Ok(port);
        }
    }
    anyhow::bail!("could not find a free port")
}

/// Build the spec for one server instance.
///
/// The contract's flags are appended here and nowhere else, so a learner's program and the
/// reference see the same command line apart from the two flags `s_server` needs to be
/// pinned to TLS 1.3 and kept quiet.
pub fn build_spec(
    def: &ServerDef,
    tmp: &Path,
    port: u16,
    material: &Material,
    options: &ServerOptions,
    boot_timeout: Duration,
) -> Result<ServerSpec> {
    let ph = Placeholders {
        port,
        cert: material.cert_pem.clone(),
        key: material.key_pem.clone(),
        tmp: tmp.to_path_buf(),
    };
    let mut argv: Vec<String> = match def.kind {
        ServerKind::Reference => reference::program()?,
        ServerKind::External => def.command.iter().map(|a| ph.apply(a)).collect(),
    };
    argv.extend(contract_flags(port, material, options));
    if def.kind == ServerKind::Reference {
        argv.extend(reference::extra_flags(options));
        // `s_server -cert` reads exactly one certificate out of the PEM, so the reference
        // needs to be told separately to send the intermediates that are already in the
        // same file. A hand-written server is expected to send everything the file holds,
        // which is why the chain stage only requires a leaf-first list of valid
        // certificates rather than a fixed length.
        if let Some(issuers) = &material.issuers_pem {
            argv.push("-cert_chain".to_string());
            argv.push(issuers.to_string_lossy().to_string());
        }
        argv.extend(def.extra_args.iter().map(|a| ph.apply(a)));
    }
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
    Ok(ServerSpec {
        name: def.name.clone(),
        argv,
        cwd,
        env,
        port,
        cert: material.cert_pem.clone(),
        key: material.key_pem.clone(),
        tmp: tmp.to_path_buf(),
        boot_timeout,
        options: options.clone(),
    })
}

/// How many connections the boot probe uses up before a test makes its first one.
pub const BOOT_PROBE_CONNECTIONS: u32 = 1;

/// The flags every server under test is given, in the order the contract writes them.
pub fn contract_flags(port: u16, material: &Material, options: &ServerOptions) -> Vec<String> {
    let mut flags = vec![
        "-accept".to_string(),
        port.to_string(),
        "-cert".to_string(),
        material.cert_pem.to_string_lossy().to_string(),
        "-key".to_string(),
        material.key_pem.to_string_lossy().to_string(),
        "-rev".to_string(),
    ];
    if let Some(n) = options.naccept {
        flags.push("-naccept".to_string());
        // The boot probe in `ServerHandle::wait_for_port` opens a TCP connection and closes
        // it again, and the server accepts it like any other. `naccept` counts the
        // connections a *test* will make, so the flag carries one more.
        flags.push((n + BOOT_PROBE_CONNECTIONS).to_string());
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::certs::{CertKind, CertStore, KeyKind};

    #[test]
    fn free_port_is_usable_and_never_4433() {
        let p = free_port().expect("free port");
        assert_ne!(p, 4433);
        let l = TcpListener::bind(("127.0.0.1", p)).expect("bind the port we were given");
        drop(l);
    }

    #[test]
    fn the_contract_flags_are_exactly_what_the_track_documents() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CertStore::new(dir.path()).expect("store");
        let m = store.get(CertKind::Leaf(KeyKind::EcdsaP256)).expect("cert");
        let flags = contract_flags(5555, &m, &ServerOptions::default());
        assert_eq!(flags[0], "-accept");
        assert_eq!(flags[1], "5555");
        assert_eq!(flags[2], "-cert");
        assert_eq!(flags[4], "-key");
        assert_eq!(flags[6], "-rev");
        assert_eq!(flags.len(), 7, "no -naccept unless a test asks for one");
        // `with_naccept(3)` means "this test makes three connections"; the flag carries a
        // fourth for the boot probe.
        let flags = contract_flags(1, &m, &ServerOptions::default().with_naccept(3));
        assert_eq!(&flags[7..], &["-naccept".to_string(), "4".to_string()]);
    }

    #[test]
    fn a_server_that_exits_immediately_is_reported() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = CertStore::new(dir.path()).expect("store");
        let m = store.get(CertKind::Leaf(KeyKind::EcdsaP256)).expect("cert");
        let spec = ServerSpec {
            name: "false".into(),
            argv: vec!["/bin/false".into()],
            cwd: dir.path().to_path_buf(),
            env: vec![],
            port: free_port().expect("port"),
            cert: m.cert_pem.clone(),
            key: m.key_pem.clone(),
            tmp: dir.path().to_path_buf(),
            boot_timeout: Duration::from_millis(2000),
            options: ServerOptions::default(),
        };
        let err = match ServerHandle::start(spec) {
            Ok(_) => panic!("/bin/false must not look like a running server"),
            Err(e) => e,
        };
        assert!(format!("{err}").contains("exited with status 1"), "{err}");
    }
}
