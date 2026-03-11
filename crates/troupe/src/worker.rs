//! Worker-side utilities for the anonymous worker protocol.
//!
//! Workers use UUID-based flat files instead of named directories:
//! - `<uuid>.ready.json` - Worker writes to signal availability
//! - `<uuid>.task.json` - Daemon writes to assign task
//! - `<uuid>.response.json` - Worker writes to complete task
//!
//! # Lifecycle
//!
//! The typical worker loop is:
//!
//! ```ignore
//! loop {
//!     let mut guard = announce_ready(&pool_root, None)?;
//!     loop {
//!         match wait_for_assignment(&mut watcher, &mut guard, timeout) {
//!             Ok(assignment) => { /* process, break to outer loop */ }
//!             Err(WaitError::Io(_)) => continue,  // timeout, retry same UUID
//!             Err(WaitError::Stopped) => return,   // guard cancels on drop
//!         }
//!     }
//! }
//! ```
//!
//! For one-shot usage, [`wait_for_task`] combines announce + wait.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use uuid::Uuid;

use crate::constants::{AGENTS_DIR, ready_path, response_path, task_path};
use crate::verified_watcher::{VerifiedWatcher, WaitError};

/// Result of waiting for a task.
#[derive(Debug)]
pub struct TaskAssignment {
    /// UUID for this task cycle (used to write response).
    pub uuid: String,
    /// Raw task content from the daemon.
    pub content: String,
}

/// RAII guard for a ready announcement.
///
/// Cancels the ready file on drop unless the assignment was successfully
/// received (at which point the daemon owns the lifecycle).
pub struct ReadyGuard {
    pool_root: PathBuf,
    uuid: String,
    active: bool,
}

impl ReadyGuard {
    /// The UUID for this ready announcement.
    #[must_use]
    pub fn uuid(&self) -> &str {
        &self.uuid
    }
}

impl Drop for ReadyGuard {
    fn drop(&mut self) {
        if self.active {
            let agents_dir = self.pool_root.join(AGENTS_DIR);
            let _ = fs::remove_file(ready_path(&agents_dir, &self.uuid));
        }
    }
}

/// Signal availability to the daemon and return a guard for this session.
///
/// Writes `<uuid>.ready.json` with optional metadata. The daemon will see
/// this and register the worker. Call [`wait_for_assignment`] to wait for
/// the daemon to assign a task.
///
/// The returned [`ReadyGuard`] cancels the ready file on drop, preventing
/// the daemon from assigning tasks to a dead worker.
///
/// # Errors
///
/// Returns an error if the ready file cannot be written.
pub fn announce_ready(pool_root: &Path, name: Option<&str>) -> Result<ReadyGuard, io::Error> {
    let agents_dir = pool_root.join(AGENTS_DIR);
    let uuid = Uuid::new_v4().to_string();
    let ready = ready_path(&agents_dir, &uuid);
    let metadata = name.map_or_else(|| "{}".to_string(), |n| format!(r#"{{"name":"{n}"}}"#));
    fs::write(&ready, &metadata).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("[E058] failed to write ready file {}: {e}", ready.display()),
        )
    })?;
    Ok(ReadyGuard {
        pool_root: pool_root.to_path_buf(),
        uuid,
        active: true,
    })
}

/// Wait for a task assignment for a previously announced UUID.
///
/// Blocks until `<uuid>.task.json` appears, or until the timeout expires.
/// On success, defuses the guard (the daemon now owns cleanup). On failure,
/// the guard remains active so the caller can retry or let it drop.
///
/// # Errors
///
/// Returns `WaitError::Stopped` if the pool was stopped.
/// Returns `WaitError::Io` on timeout or I/O failure.
pub fn wait_for_assignment(
    watcher: &mut VerifiedWatcher,
    guard: &mut ReadyGuard,
    timeout: Option<Duration>,
) -> Result<TaskAssignment, WaitError> {
    let agents_dir = guard.pool_root.join(AGENTS_DIR);
    let task = task_path(&agents_dir, &guard.uuid);

    let result = match timeout {
        Some(t) => watcher.wait_for_file_with_timeout(&task, t),
        None => watcher.wait_for_file(&task),
    };
    result?;

    let content = fs::read_to_string(&task).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!("[E059] failed to read task file {}: {e}", task.display()),
        )
    })?;

    // Defuse: the daemon assigned the task and will clean up all files
    // for this UUID after reading the response.
    guard.active = false;

    Ok(TaskAssignment {
        uuid: guard.uuid.clone(),
        content,
    })
}

/// Wait for a task assignment from the daemon (convenience wrapper).
///
/// Combines [`announce_ready`] + [`wait_for_assignment`] into one call.
/// Generates a fresh UUID each time. The [`ReadyGuard`] handles cleanup
/// automatically on success or failure.
///
/// For callers that retry in a loop with timeouts, use [`announce_ready`]
/// and [`wait_for_assignment`] directly to avoid generating a new UUID
/// per retry.
///
/// # Errors
///
/// Returns `WaitError::Stopped` if the pool was stopped.
/// Returns `WaitError::Io` if:
/// - File operations fail (writing ready file, reading task file)
/// - Timeout is exceeded waiting for task
pub fn wait_for_task(
    watcher: &mut VerifiedWatcher,
    pool_root: &Path,
    name: Option<&str>,
    timeout: Option<Duration>,
) -> Result<TaskAssignment, WaitError> {
    let mut guard = announce_ready(pool_root, name)?;
    wait_for_assignment(watcher, &mut guard, timeout)
    // On Ok: guard is defused, drop is a no-op.
    // On Err: guard is active, drop cancels the ready file.
}

/// Write a response for a completed task.
///
/// The daemon will clean up all files for this UUID after reading the response.
///
/// # Errors
///
/// Returns an error if the response file cannot be written.
pub fn write_response(pool_root: &Path, uuid: &str, content: &str) -> io::Result<()> {
    let agents_dir = pool_root.join(AGENTS_DIR);
    let path = response_path(&agents_dir, uuid);
    fs::write(&path, content).map_err(|e| {
        io::Error::new(
            e.kind(),
            format!(
                "[E060] failed to write response file {}: {e}",
                path.display()
            ),
        )
    })
}

#[cfg(test)]
#[expect(clippy::expect_used)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_dir(name: &str) -> PathBuf {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join(".test-data")
            .join("worker")
            .join(name);
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("create test dir");
        dir
    }

    #[test]
    fn write_response_creates_file() {
        let pool_root = test_dir("write_response");
        let agents_dir = pool_root.join(AGENTS_DIR);
        fs::create_dir_all(&agents_dir).expect("create agents dir");

        let uuid = "test-uuid-123";
        write_response(&pool_root, uuid, r#"{"result": "ok"}"#).expect("write response");

        let path = response_path(&agents_dir, uuid);
        assert!(path.exists());
        let content = fs::read_to_string(&path).expect("read response");
        assert_eq!(content, r#"{"result": "ok"}"#);
    }

    #[test]
    fn announce_ready_creates_file() {
        let pool_root = test_dir("announce_ready");
        let agents_dir = pool_root.join(AGENTS_DIR);
        fs::create_dir_all(&agents_dir).expect("create agents dir");

        let guard = announce_ready(&pool_root, Some("test-worker")).expect("announce");
        let ready = ready_path(&agents_dir, guard.uuid());
        assert!(ready.exists());
    }

    #[test]
    fn ready_guard_cancels_on_drop() {
        let pool_root = test_dir("guard_cancel");
        let agents_dir = pool_root.join(AGENTS_DIR);
        fs::create_dir_all(&agents_dir).expect("create agents dir");

        let uuid;
        {
            let guard = announce_ready(&pool_root, None).expect("announce");
            uuid = guard.uuid().to_string();
            let ready = ready_path(&agents_dir, &uuid);
            assert!(
                ready.exists(),
                "ready file should exist while guard is live"
            );
        }
        // Guard dropped — file should be removed.
        let ready = ready_path(&agents_dir, &uuid);
        assert!(!ready.exists(), "ready file should be removed after drop");
    }
}
