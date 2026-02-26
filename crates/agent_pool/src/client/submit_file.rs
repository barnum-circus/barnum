//! File-based task submission for sandboxed environments.
//!
//! When Unix sockets are blocked (e.g., in sandboxed environments),
//! this provides an alternative submission mechanism using only file I/O.
//!
//! # Protocol
//!
//! 1. Submitter writes `<pool>/pending/<id>.request.json` with the payload
//! 2. Daemon detects the new request and dispatches it to an agent
//! 3. When the agent completes, daemon writes `<pool>/pending/<id>.response.json`
//! 4. Submitter reads the response and cleans up both files
//!
//! # Cleanup
//!
//! - Submitter deletes both files after reading the response
//! - If submitter is killed, files may be orphaned (daemon could clean up stale files)

use super::payload::Payload;
use super::{DEFAULT_POOL_READY_TIMEOUT, wait_for_pool_ready};
use crate::constants::{PENDING_DIR, REQUEST_SUFFIX, RESPONSE_SUFFIX};
use crate::response::Response;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Default timeout for file-based submission (5 minutes).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Poll interval when waiting for response.
const POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Submit a task using file-based protocol and wait for the result.
///
/// This is an alternative to [`crate::submit`] for environments where
/// Unix sockets are blocked (e.g., sandboxes).
///
/// # Protocol
///
/// 1. Writes `<root>/pending/<id>.request.json` with the payload
/// 2. Polls for `<root>/pending/<id>.response.json`
/// 3. Returns the response and cleans up both files
///
/// # Errors
///
/// Returns an error if:
/// - The pending directory doesn't exist (daemon not ready)
/// - The request file cannot be written
/// - The response times out
/// - The response contains invalid JSON
pub fn submit_file(root: impl AsRef<Path>, payload: &Payload) -> io::Result<Response> {
    submit_file_with_timeout(root, payload, DEFAULT_TIMEOUT)
}

/// Submit a task with a custom timeout.
///
/// # Errors
///
/// Returns an error if:
/// - The pool is not ready within the default ready timeout
/// - The request file cannot be written
/// - The response times out (using the provided timeout)
/// - The response contains invalid JSON
pub fn submit_file_with_timeout(
    root: impl AsRef<Path>,
    payload: &Payload,
    timeout: Duration,
) -> io::Result<Response> {
    let root = root.as_ref();
    let pending_dir = root.join(PENDING_DIR);

    // Wait for daemon to be ready
    wait_for_pool_ready(root, DEFAULT_POOL_READY_TIMEOUT)?;

    // Generate unique submission ID
    let submission_id = Uuid::new_v4().to_string();

    // Flat files directly in pending directory (no subdirectory creation!)
    let request_path = pending_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = pending_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));

    // Write request file with serialized payload (atomic: write temp in /tmp, rename)
    let content = serde_json::to_string(payload)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    // Use submission_id for uniqueness (already a UUID)
    let temp_path = PathBuf::from("/tmp").join(format!("gsd-req-{submission_id}.tmp"));
    fs::write(&temp_path, &content)?;
    fs::rename(&temp_path, &request_path)?;

    // Poll for response
    let start = Instant::now();
    loop {
        if response_path.exists() {
            // Read and parse response
            let response_content = fs::read_to_string(&response_path)?;

            // Clean up both files
            let _ = fs::remove_file(&request_path);
            let _ = fs::remove_file(&response_path);

            let response: Response = serde_json::from_str(&response_content)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            return Ok(response);
        }

        // Check timeout
        if start.elapsed() > timeout {
            // Clean up on timeout
            let _ = fs::remove_file(&request_path);
            let _ = fs::remove_file(&response_path);

            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("file-based submit timed out after {timeout:?}"),
            ));
        }

        thread::sleep(POLL_INTERVAL);
    }
}

/// Clean up a pending submission's files.
///
/// Call this if you need to abandon a submission (e.g., on interrupt).
///
/// # Errors
///
/// Returns an error if file removal fails (though errors are typically ignored).
pub fn cleanup_submission(root: impl AsRef<Path>, submission_id: &str) -> io::Result<()> {
    let pending_dir = root.as_ref().join(PENDING_DIR);
    let request_path = pending_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = pending_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));
    let _ = fs::remove_file(&request_path);
    let _ = fs::remove_file(&response_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::constants::PENDING_DIR;

    #[test]
    fn pending_dir_constant() {
        assert_eq!(PENDING_DIR, "pending");
    }
}
