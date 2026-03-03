//! Worker-side utilities for the anonymous worker protocol.
//!
//! Workers use UUID-based flat files instead of named directories:
//! - `<uuid>.ready.json` - Worker writes to signal availability
//! - `<uuid>.task.json` - Daemon writes to assign task
//! - `<uuid>.response.json` - Worker writes to complete task
//!
//! Each task cycle uses a fresh UUID. This eliminates race conditions
//! and simplifies the protocol.

use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use uuid::Uuid;

use crate::constants::{AGENTS_DIR, ready_path, response_path, task_path};
use crate::verified_watcher::VerifiedWatcher;

/// Result of waiting for a task.
#[derive(Debug)]
pub struct TaskAssignment {
    /// UUID for this task cycle (used to write response).
    pub uuid: String,
    /// Raw task content from the daemon.
    pub content: String,
}

/// Wait for a task assignment from the daemon.
///
/// This function:
/// 1. Generates a fresh UUID
/// 2. Writes `<uuid>.ready.json` to signal availability
/// 3. Waits for `<uuid>.task.json` using a verified watcher
/// 4. Returns the UUID and task content
///
/// The optional `name` parameter is included in the ready file for debugging.
///
/// # Errors
///
/// Returns an error if:
/// - File operations fail (writing ready file, reading task file)
/// - Timeout is exceeded waiting for task
/// - Watcher cannot be created
pub fn wait_for_task(
    pool_root: &Path,
    name: Option<&str>,
    timeout: Option<Duration>,
) -> io::Result<TaskAssignment> {
    let agents_dir = pool_root.join(AGENTS_DIR);
    let uuid = Uuid::new_v4().to_string();

    let ready = ready_path(&agents_dir, &uuid);
    let task = task_path(&agents_dir, &uuid);

    // Write ready file with optional metadata
    let metadata = name.map_or_else(|| "{}".to_string(), |n| format!(r#"{{"name":"{n}"}}"#));
    fs::write(&ready, &metadata)?;

    // Wait for task file using VerifiedWatcher
    let mut watcher = VerifiedWatcher::new(&agents_dir, agents_dir.clone())?;
    watcher.wait_for(&task, timeout)?;

    let content = fs::read_to_string(&task)?;
    Ok(TaskAssignment { uuid, content })
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
    fs::write(&path, content)
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
}
