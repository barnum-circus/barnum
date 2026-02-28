//! File-based task submission for sandboxed environments.
//!
//! When Unix sockets are blocked (e.g., in sandboxed environments),
//! this provides an alternative submission mechanism using only file I/O.
//!
//! # Protocol
//!
//! 1. Submitter writes `<pool>/submissions/<id>.request.json` with the payload
//! 2. Daemon detects the new request and dispatches it to an agent
//! 3. When the agent completes, daemon writes `<pool>/submissions/<id>.response.json`
//! 4. Submitter reads the response and cleans up both files
//!
//! # Cleanup
//!
//! - Submitter deletes both files after reading the response
//! - If submitter is killed, files may be orphaned (daemon could clean up stale files)

use super::payload::Payload;
use super::{DEFAULT_POOL_READY_TIMEOUT, wait_for_pool_ready};
use crate::constants::{REQUEST_SUFFIX, RESPONSE_SUFFIX, SUBMISSIONS_DIR};
use crate::response::Response;
use std::fs;
use std::io;
use std::path::Path;
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
/// 1. Writes `<root>/submissions/<id>.request.json` with the payload
/// 2. Polls for `<root>/submissions/<id>.response.json`
/// 3. Returns the response and cleans up both files
///
/// # Errors
///
/// Returns an error if:
/// - The submissions directory doesn't exist (daemon not ready)
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
    let submissions_dir = root.join(SUBMISSIONS_DIR);

    // Wait for daemon to be ready
    wait_for_pool_ready(root, DEFAULT_POOL_READY_TIMEOUT)?;

    // Generate unique submission ID
    let submission_id = Uuid::new_v4().to_string();

    // Flat files directly in submissions directory (no subdirectory creation!)
    let request_path = submissions_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = submissions_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));

    // Write request file with serialized payload (atomic: write temp in same dir, rename)
    let content = serde_json::to_string(payload)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    // Temp file in same directory to ensure same filesystem for rename
    let temp_path = submissions_dir.join(format!(".{submission_id}.tmp"));
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
    let submissions_dir = root.as_ref().join(SUBMISSIONS_DIR);
    let request_path = submissions_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = submissions_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));
    let _ = fs::remove_file(&request_path);
    let _ = fs::remove_file(&response_path);
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::constants::SUBMISSIONS_DIR;

    #[test]
    fn submissions_dir_constant() {
        assert_eq!(SUBMISSIONS_DIR, "submissions");
    }
}
