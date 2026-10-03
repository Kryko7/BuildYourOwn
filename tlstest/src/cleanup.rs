//! Making sure an interrupted run does not leave `s_server` processes or scratch data behind.
//!
//! Every server is started in its own process group (so `./your_program.sh` and whatever it
//! execs die together), which also means a `Ctrl-C` in the terminal never reaches it: the
//! terminal signals only the harness's own group, and `Drop` does not run when a signal ends
//! the process. Without this module an interrupted `--validate` left an `s_server` listening
//! forever and a `tlstest-<pid>` directory in the temp dir.
//!
//! So the harness keeps a registry of the process groups and directories it owns, and
//! [`install_signal_handler`] turns `SIGINT`, `SIGTERM` and `SIGHUP` into "kill every
//! registered group, delete every registered directory, exit". A run killed with `SIGKILL`
//! cannot clean up after itself; [`sweep_abandoned`] is what the next run does about it.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

static GROUPS: Mutex<BTreeSet<i32>> = Mutex::new(BTreeSet::new());
static DIRS: Mutex<BTreeSet<PathBuf>> = Mutex::new(BTreeSet::new());

/// The prefix of a run's scratch directory under the system temp dir: `tlstest-<pid>`.
pub const SCRATCH_PREFIX: &str = "tlstest-";

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
/// mask, and the one watcher thread collects them with `sigwait`. A blocked mask is
/// inherited across `exec`, so every child the harness starts has to clear it with
/// [`reset_signal_mask_in_child`].
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
        .name("tlstest-signals".into())
        .spawn(move || {
            let mut sig: libc::c_int = 0;
            // SAFETY: `set` is a valid, initialised sigset_t owned by this closure.
            let rc = unsafe { libc::sigwait(&set, &mut sig) };
            if rc != 0 {
                return;
            }
            eprintln!("\ntlstest: interrupted, stopping the server and removing scratch data");
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

/// Give a child process an empty signal mask, from inside `pre_exec`.
///
/// [`install_signal_handler`] blocks `SIGINT`, `SIGTERM` and `SIGHUP` in every thread of
/// the harness, and a blocked mask survives `fork` and `exec`. Without this, an `s_server`
/// started afterwards would sit through the `SIGTERM` [`crate::server::ServerHandle::stop`]
/// sends and only die to the `SIGKILL` two seconds later — on every server restart.
pub fn reset_signal_mask_in_child() -> std::io::Result<()> {
    // SAFETY: sigemptyset and pthread_sigmask are async-signal-safe, which is all a
    // pre_exec closure may call; the set is a local.
    unsafe {
        let mut empty: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut empty);
        if libc::pthread_sigmask(libc::SIG_SETMASK, &empty, std::ptr::null_mut()) != 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

/// The pid in a scratch directory name (`tlstest-<pid>`).
fn owner_pid(name: &str) -> Option<u32> {
    name.strip_prefix(SCRATCH_PREFIX)?.parse().ok()
}

/// Kill whatever is still running out of, and delete, the scratch directories of `tlstest`
/// runs that are no longer running — what a run killed with `SIGKILL` leaves behind.
pub fn sweep_abandoned() {
    let tmp = std::env::temp_dir();
    let Ok(entries) = std::fs::read_dir(&tmp) else {
        return;
    };
    let me = std::process::id();
    let mut dead: Vec<PathBuf> = Vec::new();
    for entry in entries.flatten() {
        let Some(pid) = owner_pid(&entry.file_name().to_string_lossy()) else {
            continue;
        };
        if pid == me || Path::new(&format!("/proc/{pid}")).exists() {
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

/// `SIGKILL` every process whose command line names a path under one of `dirs` — an
/// `s_server` is always started with `-cert <scratch>/certs/...`.
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
            // The directory itself or anything under it; never a sibling whose name merely
            // starts the same way (`tlstest-12` must not match `tlstest-123`).
            dirs.iter()
                .any(|d| arg.ends_with(d.as_str()) || arg.contains(&format!("{d}/")))
        });
        if hit {
            // SAFETY: kill on a pid read from /proc; the kernel refuses anything this user
            // does not own.
            unsafe {
                libc::kill(pid, libc::SIGKILL);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_names_carry_their_owner() {
        assert_eq!(owner_pid("tlstest-1234"), Some(1234));
        assert_eq!(owner_pid("tlstest-x"), None);
        assert_eq!(owner_pid("disttest-12"), None);
    }

    #[test]
    fn the_registry_forgets_what_it_is_told_to() {
        register_group(987_654);
        unregister_group(987_654);
        assert!(!GROUPS.lock().expect("lock").contains(&987_654));
        let p = Path::new("/nonexistent/tlstest-registry-test");
        register_dir(p);
        unregister_dir(p);
        assert!(!DIRS.lock().expect("lock").contains(p));
    }
}
