//! Making sure a run that is interrupted does not leave servers or scratch data behind.
//!
//! Every node is started in its own process group, which keeps a shell wrapper and the
//! server it execs dying together, but it also means a `Ctrl-C` in the terminal never
//! reaches them: the terminal only signals the harness's own group. `Drop` does not run
//! when a signal ends the process either. So the harness keeps a registry of the process
//! groups and scratch directories it owns, and [`install_signal_handler`] turns `SIGINT`,
//! `SIGTERM` and `SIGHUP` into "kill every registered group, delete every registered
//! directory, exit".
//!
//! A run killed with `SIGKILL` cannot clean up at all; [`sweep_abandoned`] is what the next
//! run does about it: it kills every process still running out of a dead run's scratch
//! directory, then deletes the directory.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static GROUPS: Mutex<BTreeSet<i32>> = Mutex::new(BTreeSet::new());
static DIRS: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// Remember a process group the harness created, so an interrupted run can kill it.
pub fn register_group(pgid: i32) {
    if pgid > 1 {
        if let Ok(mut g) = GROUPS.lock() {
            g.insert(pgid);
        }
    }
}

/// Forget a process group once it has been killed and reaped.
pub fn unregister_group(pgid: i32) {
    if let Ok(mut g) = GROUPS.lock() {
        g.remove(&pgid);
    }
}

/// Remember a scratch directory an interrupted run must delete.
pub fn register_dir(dir: &Path) {
    if let Ok(mut d) = DIRS.lock() {
        d.insert(dir.to_path_buf());
    }
}

/// Forget a scratch directory (it was deleted, or `--keep-tmp` wants it kept).
pub fn unregister_dir(dir: &Path) {
    if let Ok(mut d) = DIRS.lock() {
        d.remove(dir);
    }
}

/// Kill every registered process group and delete every registered directory.
pub fn kill_everything() {
    let groups: Vec<i32> = GROUPS
        .lock()
        .map(|g| g.iter().copied().collect())
        .unwrap_or_default();
    for pgid in groups {
        // SAFETY: killpg on a process group this harness created and has not yet reaped.
        unsafe {
            libc::killpg(pgid, libc::SIGKILL);
        }
    }
    let dirs: Vec<PathBuf> = DIRS
        .lock()
        .map(|d| d.iter().cloned().collect())
        .unwrap_or_default();
    for d in dirs {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// Route `SIGINT`, `SIGTERM` and `SIGHUP` to a thread that cleans up and exits.
///
/// Must be called before any other thread exists (before the tokio runtime is built): the
/// signals are blocked in the calling thread, every thread created afterwards inherits that
/// mask, and the one watcher thread collects them with `sigwait`.
///
/// A blocked mask survives `fork` and `exec`, and the standard library does *not* reset
/// it, so every spawn site must go through [`unblock_signals_in_child`]: a node started
/// with `SIGTERM` blocked ignores the polite stop and every shutdown waits for `SIGKILL`.
pub fn install_signal_handler() {
    // SAFETY: plain libc signal-set manipulation on a local, zero-initialised sigset_t.
    let set = unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        libc::sigaddset(&mut set, libc::SIGINT);
        libc::sigaddset(&mut set, libc::SIGTERM);
        libc::sigaddset(&mut set, libc::SIGHUP);
        if libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut()) != 0 {
            return;
        }
        set
    };
    let spawned = std::thread::Builder::new()
        .name("disttest-signals".into())
        .spawn(move || {
            let mut sig: libc::c_int = 0;
            // SAFETY: `set` is a valid, initialised sigset_t owned by this closure.
            let rc = unsafe { libc::sigwait(&set, &mut sig) };
            if rc != 0 {
                return;
            }
            eprintln!("\ndisttest: interrupted, stopping every node and removing scratch data");
            kill_everything();
            std::process::exit(128 + sig);
        });
    if spawned.is_err() {
        // Without a watcher, blocked signals would make the harness unkillable by Ctrl-C.
        // SAFETY: as above.
        unsafe {
            libc::pthread_sigmask(libc::SIG_UNBLOCK, &set, std::ptr::null_mut());
        }
    }
}

/// Give a child an empty signal mask, whatever the spawning thread had blocked.
pub fn unblock_signals_in_child(cmd: &mut std::process::Command) {
    use std::os::unix::process::CommandExt;
    // SAFETY: the hook only calls sigemptyset and pthread_sigmask, both async-signal-safe,
    // on a local set, between fork and exec.
    unsafe {
        cmd.pre_exec(empty_mask);
    }
}

/// [`unblock_signals_in_child`] for a tokio command.
pub fn unblock_signals_in_tokio_child(cmd: &mut tokio::process::Command) {
    // SAFETY: as above.
    unsafe {
        cmd.pre_exec(empty_mask);
    }
}

fn empty_mask() -> std::io::Result<()> {
    // SAFETY: async-signal-safe calls on a local, zero-initialised sigset_t.
    unsafe {
        let mut set: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut set);
        if libc::pthread_sigmask(libc::SIG_SETMASK, &set, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The pid in a scratch directory name (`disttest-<pid>` or `disttest-capture-<pid>`).
fn owner_pid(name: &str) -> Option<u32> {
    let rest = name.strip_prefix("disttest-")?;
    let rest = rest.strip_prefix("capture-").unwrap_or(rest);
    rest.parse().ok()
}

/// Kill whatever is still running out of, and delete, the scratch directories of
/// `disttest` runs that are no longer running.
///
/// A run that was killed with `SIGKILL` leaves its nodes running (they live in their own
/// process groups, so nothing else reaches them) and their data directories behind, each
/// holding a preallocated 64 MB write-ahead log and a pair of bound ports.
pub fn sweep_abandoned() {
    let tmp = std::env::temp_dir();
    let Ok(entries) = std::fs::read_dir(&tmp) else {
        return;
    };
    let me = std::process::id();
    let mut dead: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = owner_pid(&name.to_string_lossy()) else {
            continue;
        };
        if pid == me {
            continue;
        }
        // `/proc/<pid>` is the cheapest "is anyone still using this" there is.
        if Path::new(&format!("/proc/{pid}")).exists() {
            continue;
        }
        dead.push(entry.path());
    }
    if dead.is_empty() {
        return;
    }
    kill_processes_under(&dead);
    for d in dead {
        let _ = std::fs::remove_dir_all(d);
    }
}

/// `SIGKILL` every process whose command line names a path under one of `dirs`.
fn kill_processes_under(dirs: &[PathBuf]) {
    let Ok(procs) = std::fs::read_dir("/proc") else {
        return;
    };
    let me = std::process::id();
    let dirs: Vec<String> = dirs
        .iter()
        .map(|d| d.to_string_lossy().into_owned())
        .collect();
    for p in procs.flatten() {
        let Some(pid) = p.file_name().to_str().and_then(|s| s.parse::<i32>().ok()) else {
            continue;
        };
        if pid <= 1 || pid as u32 == me {
            continue;
        }
        let Ok(raw) = std::fs::read(p.path().join("cmdline")) else {
            continue;
        };
        let hit = raw.split(|b| *b == 0).any(|arg| {
            let arg = String::from_utf8_lossy(arg);
            // The directory itself, or anything under it; never a sibling whose name merely
            // starts the same way (`disttest-12` must not match `disttest-123`).
            dirs.iter()
                .any(|d| arg.ends_with(d.as_str()) || arg.contains(&format!("{d}/")))
        });
        if hit {
            // SAFETY: kill on a pid read from /proc; the kernel refuses (EPERM) anything
            // this user does not own.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A child spawned from a thread that has INT/TERM/HUP blocked starts with an empty
    /// mask once the hook is applied, through both `std` and `tokio`. Without it the mask
    /// is inherited, nodes ignore the polite `SIGTERM` of `NodeHandle::stop`, and every
    /// stop waits for the `SIGKILL`.
    #[test]
    fn children_do_not_inherit_the_blocked_signals() {
        let masks = std::thread::spawn(|| {
            // SAFETY: plain signal-set manipulation on this test's own thread.
            unsafe {
                let mut set: libc::sigset_t = std::mem::zeroed();
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGINT);
                libc::sigaddset(&mut set, libc::SIGTERM);
                libc::sigaddset(&mut set, libc::SIGHUP);
                libc::pthread_sigmask(libc::SIG_BLOCK, &set, std::ptr::null_mut());
            }
            let script = ["-c", "grep SigBlk /proc/self/status"];
            let mut std_cmd = std::process::Command::new("sh");
            std_cmd.args(script);
            unblock_signals_in_child(&mut std_cmd);
            let from_std = std_cmd.output().expect("std spawn");
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("runtime");
            let from_tokio = rt
                .block_on(async {
                    let mut c = tokio::process::Command::new("sh");
                    c.args(script);
                    unblock_signals_in_tokio_child(&mut c);
                    c.output().await
                })
                .expect("tokio spawn");
            [from_std, from_tokio].map(|o| String::from_utf8_lossy(&o.stdout).to_string())
        })
        .join()
        .expect("thread");
        for m in masks {
            assert_eq!(m.trim(), "SigBlk:\t0000000000000000", "{m:?}");
        }
    }

    #[test]
    fn scratch_names_carry_their_owner() {
        assert_eq!(owner_pid("disttest-1234"), Some(1234));
        assert_eq!(owner_pid("disttest-capture-77"), Some(77));
        assert_eq!(owner_pid("disttest-capture-x"), None);
        assert_eq!(owner_pid("shelltest-12"), None);
    }

    #[test]
    fn a_process_running_out_of_a_dead_scratch_dir_is_killed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let marker = dir.path().join("data");
        let sibling = format!("{}0/data", dir.path().display());
        let spawn = |arg: &str| {
            // Two commands, so the shell cannot exec `sleep` and drop its own command line.
            std::process::Command::new("sh")
                .args(["-c", "sleep 30; true"])
                .arg(arg)
                .spawn()
                .expect("spawn sh")
        };
        let mut under = spawn(&marker.to_string_lossy());
        let mut beside = spawn(&sibling);
        std::thread::sleep(std::time::Duration::from_millis(100));
        kill_processes_under(&[dir.path().to_path_buf()]);
        use std::os::unix::process::ExitStatusExt;
        let status = under.wait().expect("wait");
        assert_eq!(
            status.signal(),
            Some(libc::SIGKILL),
            "the sweep must kill it"
        );
        assert!(
            beside.try_wait().expect("try_wait").is_none(),
            "a sibling directory with a longer name is not ours"
        );
        let _ = beside.kill();
        let _ = beside.wait();
    }
}
