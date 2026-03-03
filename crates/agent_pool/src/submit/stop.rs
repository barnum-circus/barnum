//! Stop a running agent pool daemon.

// TEMPORARY: Allow panic for debugging
#![allow(clippy::panic, clippy::missing_panics_doc, unreachable_code)]

use crate::constants::{LOCK_FILE, STATUS_FILE};
use std::path::Path;
use std::time::Duration;
use std::{fs, io, thread};
use tracing::warn;

/// Stop a running agent pool daemon.
///
/// First writes "stop" to the status file to trigger graceful shutdown,
/// waits briefly for the daemon to exit, then sends SIGTERM as a fallback.
///
/// # Errors
///
/// Returns an error if:
/// - No daemon is running (lock file not found)
/// - The lock file contains invalid data
/// - The process could not be terminated
pub fn stop(root: impl AsRef<Path>) -> io::Result<()> {
    let root = root.as_ref();
    let lock_path = root.join(LOCK_FILE);
    let status_path = root.join(STATUS_FILE);

    if !lock_path.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no daemon running (lock file not found)",
        ));
    }

    let pid_str = fs::read_to_string(&lock_path)?;
    let pid: u32 = pid_str
        .trim()
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    // Signal graceful shutdown by writing "stop" to status file
    warn!(
        pool = %root.display(),
        target_pid = pid,
        our_pid = std::process::id(),
        "STOP: writing 'stop' to status file"
    );

    // TEMPORARY: Capture backtrace and write to status file
    let bt = std::backtrace::Backtrace::force_capture();
    let marker = format!(
        "stop|pid:{}|pool:{}|BACKTRACE:\n{}",
        std::process::id(),
        root.display(),
        bt
    );
    let _ = fs::write(&status_path, &marker);

    // Give the daemon a moment to shut down gracefully
    thread::sleep(Duration::from_millis(100));

    // Check if process is still running
    if is_process_running(pid) {
        // Send SIGTERM as fallback
        terminate_process(pid)?;
    }

    Ok(())
}

#[cfg(unix)]
fn is_process_running(pid: u32) -> bool {
    // On Unix, kill with signal 0 checks if process exists without killing it
    use std::process::Command;
    Command::new("kill")
        .args(["-0", &pid.to_string()])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_process_running(pid: u32) -> bool {
    // On Windows, use tasklist to check if process exists
    use std::process::Command;
    Command::new("tasklist")
        .args(["/FI", &format!("PID eq {pid}"), "/NH"])
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains(&pid.to_string()))
        .unwrap_or(false)
}

#[cfg(unix)]
fn terminate_process(pid: u32) -> io::Result<()> {
    use std::process::Command;

    let status = Command::new("kill").arg(pid.to_string()).status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "failed to terminate process {pid}"
        )))
    }
}

#[cfg(windows)]
fn terminate_process(pid: u32) -> io::Result<()> {
    use std::process::Command;

    let status = Command::new("taskkill")
        .args(["/PID", &pid.to_string(), "/F"])
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!(
            "failed to terminate process {pid}"
        )))
    }
}
