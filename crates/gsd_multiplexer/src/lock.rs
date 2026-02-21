//! Process lock management for single-daemon enforcement.

use std::path::Path;
use std::{fs, io};

/// Acquire an exclusive lock for the daemon.
///
/// Checks if another daemon is already running by verifying the PID
/// in the lock file. If the process is dead, the stale lock is removed.
pub fn acquire_lock(lock_path: &Path) -> io::Result<()> {
    if lock_path.exists() {
        if let Ok(pid_str) = fs::read_to_string(lock_path) {
            if let Ok(pid) = pid_str.trim().parse::<u32>() {
                if is_process_running(pid) {
                    return Err(io::Error::new(
                        io::ErrorKind::AlreadyExists,
                        format!("daemon already running (PID {pid})"),
                    ));
                }
            }
        }
        fs::remove_file(lock_path)?;
    }

    fs::write(lock_path, std::process::id().to_string())?;
    Ok(())
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
