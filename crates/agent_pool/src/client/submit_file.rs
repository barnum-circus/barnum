//! File-based task submission for sandboxed environments.
//!
//! When Unix sockets are blocked (e.g., in sandboxed environments),
//! this provides an alternative submission mechanism using only file I/O.
//!
//! # Protocol
//!
//! 1. Submitter creates `<pool>/pending/<uuid>/task.json` with the payload
//! 2. Daemon detects the new task and dispatches it to an agent
//! 3. When the agent completes, daemon writes `<pool>/pending/<uuid>/response.json`
//! 4. Submitter reads the response and cleans up the directory
//!
//! # Cleanup
//!
//! - Submitter deletes the `<uuid>` directory after reading the response
//! - If submitter is killed, it tries to delete the directory on signal
//! - Daemon may clean up stale pending directories after a timeout

use super::payload::Payload;
use crate::response::Response;
use std::fs;
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};
use uuid::Uuid;

/// Directory for pending file-based submissions.
pub const PENDING_DIR: &str = "pending";

/// Task file name within a pending submission.
pub const PENDING_TASK_FILE: &str = "task.json";

/// Response file name within a pending submission.
pub const PENDING_RESPONSE_FILE: &str = "response.json";

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
/// 1. Creates `<root>/pending/<uuid>/task.json` with the payload
/// 2. Polls for `<root>/pending/<uuid>/response.json`
/// 3. Returns the response and cleans up
///
/// # Errors
///
/// Returns an error if:
/// - The pending directory cannot be created
/// - The task file cannot be written
/// - The response times out
/// - The response contains invalid JSON
pub fn submit_file(root: impl AsRef<Path>, payload: &Payload) -> io::Result<Response> {
    submit_file_with_timeout(root, payload, DEFAULT_TIMEOUT)
}

/// Submit a task with a custom timeout.
pub fn submit_file_with_timeout(
    root: impl AsRef<Path>,
    payload: &Payload,
    timeout: Duration,
) -> io::Result<Response> {
    let root = root.as_ref();
    let pending_dir = root.join(PENDING_DIR);

    // Daemon creates pending_dir after watcher starts - if it doesn't exist, daemon isn't ready
    if !pending_dir.exists() {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "daemon not ready (pending directory doesn't exist)",
        ));
    }

    // Generate unique submission ID
    let submission_id = Uuid::new_v4().to_string();
    let submission_dir = pending_dir.join(&submission_id);

    // Create submission directory
    fs::create_dir(&submission_dir)?;

    let task_path = submission_dir.join(PENDING_TASK_FILE);
    let response_path = submission_dir.join(PENDING_RESPONSE_FILE);

    // Write task file with serialized payload
    let content = serde_json::to_string(payload)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(&task_path, content)?;

    // Poll for response (task file removal just means daemon picked it up)
    let start = Instant::now();
    loop {
        if response_path.exists() {
            // Read and parse response
            let response_content = fs::read_to_string(&response_path)?;

            // Clean up submission directory
            let _ = fs::remove_dir_all(&submission_dir);

            let response: Response = serde_json::from_str(&response_content)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            return Ok(response);
        }

        // Check timeout
        if start.elapsed() > timeout {
            // Clean up on timeout
            let _ = fs::remove_dir_all(&submission_dir);

            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("file-based submit timed out after {timeout:?}"),
            ));
        }

        thread::sleep(POLL_INTERVAL);
    }
}

/// Clean up a pending submission directory.
///
/// Call this if you need to abandon a submission (e.g., on interrupt).
///
/// # Errors
///
/// Returns an error if the directory cannot be removed.
pub fn cleanup_submission(root: impl AsRef<Path>, submission_id: &str) -> io::Result<()> {
    let submission_dir = root.as_ref().join(PENDING_DIR).join(submission_id);
    if submission_dir.exists() {
        fs::remove_dir_all(&submission_dir)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pending_dir_constant() {
        assert_eq!(PENDING_DIR, "pending");
    }
}
