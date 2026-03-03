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
use crate::constants::{REQUEST_SUFFIX, RESPONSE_SUFFIX, STATUS_FILE, SUBMISSIONS_DIR};
use crate::response::Response;
use crate::verified_watcher::{VerifiedWatcher, atomic_write_str};
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;
use uuid::Uuid;

/// Default timeout for file-based submission (5 minutes).
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);

/// Default timeout for waiting for pool to become ready (10 seconds).
const POOL_READY_TIMEOUT: Duration = Duration::from_secs(10);

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
/// Uses a filesystem watcher to efficiently wait for the response instead of
/// polling with `thread::sleep`.
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
    let root = fs::canonicalize(root.as_ref())?;
    let submissions_dir = root.join(SUBMISSIONS_DIR);

    // Generate unique submission ID
    let submission_id = Uuid::new_v4().to_string();

    // File paths
    let request_path = submissions_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = submissions_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));
    let status_path = root.join(STATUS_FILE);

    // Create watcher on pool root (watches recursively to see both status and response)
    let mut watcher = VerifiedWatcher::new(&root, root.clone())?;

    // Wait for pool to become ready (returns immediately if status file exists)
    watcher.wait_for(&status_path, Some(POOL_READY_TIMEOUT))?;

    // Write request file with serialized payload (atomic via scratch/)
    let content = serde_json::to_string(payload)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    atomic_write_str(&root, &request_path, &content)?;

    // Wait for response using the same watcher (already verified)
    watcher.wait_for(&response_path, Some(timeout))?;

    // Read and parse response
    let response_content = fs::read_to_string(&response_path)?;

    // Clean up both files
    let _ = fs::remove_file(&request_path);
    let _ = fs::remove_file(&response_path);

    let response: Response = serde_json::from_str(&response_content)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    Ok(response)
}

#[cfg(test)]
mod tests {
    use crate::constants::SUBMISSIONS_DIR;

    #[test]
    fn submissions_dir_constant() {
        assert_eq!(SUBMISSIONS_DIR, "submissions");
    }
}
