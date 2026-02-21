//! Stop a running multiplexer daemon.

use crate::constants::LOCK_FILE;
use std::path::Path;
use std::{fs, io};

/// Stop a running multiplexer daemon.
///
/// Reads the PID from the lock file and sends SIGTERM to stop it.
pub fn stop(root: impl AsRef<Path>) -> io::Result<()> {
    let lock_path = root.as_ref().join(LOCK_FILE);

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

    terminate_process(pid)
}

#[cfg(unix)]
fn terminate_process(pid: u32) -> io::Result<()> {
    use std::process::Command;

    let status = Command::new("kill")
        .arg(pid.to_string())
        .status()?;

    if status.success() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("failed to terminate process {pid}"),
        ))
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
        Err(io::Error::new(
            io::ErrorKind::Other,
            format!("failed to terminate process {pid}"),
        ))
    }
}
