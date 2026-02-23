//! Process lock management for single-daemon enforcement.

use std::path::{Path, PathBuf};
use std::{fs, io};

/// An exclusive lock for the daemon.
///
/// The lock file is automatically removed when this guard is dropped.
pub struct LockGuard {
    path: PathBuf,
}

impl Drop for LockGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Acquire an exclusive lock for the daemon.
///
/// Checks if another daemon is already running by verifying the PID
/// in the lock file. If the process is dead, the stale lock is removed.
///
/// Returns a guard that releases the lock on drop.
pub fn acquire_lock(lock_path: &Path) -> io::Result<LockGuard> {
    if lock_path.exists() {
        // Check if an existing daemon is still running
        if let Ok(pid_str) = fs::read_to_string(lock_path)
            && let Ok(pid) = pid_str.trim().parse::<u32>()
            && is_process_running(pid)
        {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                format!("daemon already running (PID {pid})"),
            ));
        }
        // Stale lock - remove it
        fs::remove_file(lock_path)?;
    }

    fs::write(lock_path, std::process::id().to_string())?;
    Ok(LockGuard {
        path: lock_path.to_path_buf(),
    })
}

#[cfg(unix)]
fn is_process_running(pid: u32) -> bool {
    std::process::Command::new("kill")
        .args(["-0", &pid.to_string()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

#[cfg(not(unix))]
fn is_process_running(_pid: u32) -> bool {
    true
}

/// Check if a daemon is running for the given pool root.
///
/// Returns `true` if the lock file exists and the PID is still alive.
pub fn is_daemon_running(root: impl AsRef<Path>) -> bool {
    let lock_path = root.as_ref().join(crate::constants::LOCK_FILE);
    if let Ok(pid_str) = fs::read_to_string(&lock_path)
        && let Ok(pid) = pid_str.trim().parse::<u32>()
    {
        is_process_running(pid)
    } else {
        false
    }
}
