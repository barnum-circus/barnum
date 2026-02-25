//! Task submission to the agent pool daemon.

use super::payload::Payload;
use crate::constants::{SOCKET_NAME, STATUS_FILE};
use crate::response::Response;
use interprocess::local_socket::{GenericFilePath, Stream, prelude::*};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

/// Timeout for waiting for daemon to be ready.
const DAEMON_READY_TIMEOUT: Duration = Duration::from_secs(5);

/// Poll interval when waiting for daemon readiness.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Submit a task to the agent pool and wait for the result.
///
/// Connects to the daemon's Unix socket, sends the task, and blocks
/// until the result is available. Returns a structured [`Response`]
/// that indicates whether the task was processed successfully.
///
/// # Errors
///
/// Returns an error if:
/// - The daemon socket doesn't exist or can't be connected to
/// - Communication with the daemon fails
/// - The response contains invalid JSON
pub fn submit(root: impl AsRef<Path>, payload: &Payload) -> io::Result<Response> {
    let root = root.as_ref();
    let status_file = root.join(STATUS_FILE);

    // Wait for daemon to be ready (status file exists)
    wait_for_daemon_ready(&status_file, DAEMON_READY_TIMEOUT)?;

    let socket_path = root.join(SOCKET_NAME);

    let name = socket_path
        .to_fs_name::<GenericFilePath>()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;

    let input = serde_json::to_string(payload)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut stream =
        Stream::connect(name).map_err(|e| io::Error::new(io::ErrorKind::ConnectionRefused, e))?;
    writeln!(stream, "{}", input.len())?;
    stream.write_all(input.as_bytes())?;
    stream.flush()?;

    let mut reader = BufReader::new(stream);

    let mut len_line = String::new();
    reader.read_line(&mut len_line)?;
    let len: usize = len_line
        .trim()
        .parse()
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let mut output = vec![0u8; len];
    reader.read_exact(&mut output)?;

    let json =
        String::from_utf8(output).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    serde_json::from_str(&json).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

/// Wait for the daemon to be ready by polling for the status file.
fn wait_for_daemon_ready(status_file: &Path, timeout: Duration) -> io::Result<()> {
    let start = Instant::now();
    while !status_file.exists() {
        if start.elapsed() > timeout {
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "daemon not ready (status file not found within timeout)",
            ));
        }
        thread::sleep(POLL_INTERVAL);
    }
    Ok(())
}
