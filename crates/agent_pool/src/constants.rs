//! Shared constants for the agent pool protocol.

use std::path::{Path, PathBuf};

/// Directory containing agent subdirectories.
pub const AGENTS_DIR: &str = "agents";

/// Directory for file-based task submissions (sandboxed environments).
pub const SUBMISSIONS_DIR: &str = "submissions";

/// Directory for temporary files (atomic writes). Not watched.
pub const SCRATCH_DIR: &str = "scratch";

/// Lock file for single-daemon enforcement.
pub const LOCK_FILE: &str = "daemon.lock";

/// Socket name for IPC (file path on Unix, named pipe on Windows).
pub const SOCKET_NAME: &str = "daemon.sock";

/// Stable filename for task input (used by agents).
pub const TASK_FILE: &str = "task.json";

/// Stable filename for agent response (used by agents).
pub const RESPONSE_FILE: &str = "response.json";

/// Suffix for submission request files (flat structure).
pub const REQUEST_SUFFIX: &str = ".request.json";

/// Suffix for submission response files (flat structure).
pub const RESPONSE_SUFFIX: &str = ".response.json";

/// Status file written when daemon is ready.
/// When changed to "stop", triggers graceful daemon shutdown.
pub const STATUS_FILE: &str = "status";

// =============================================================================
// Anonymous worker file suffixes (flat files in agents/)
// =============================================================================

/// Suffix for worker ready files: `<uuid>.ready.json`
pub const READY_SUFFIX: &str = ".ready.json";

/// Suffix for worker task files: `<uuid>.task.json`
pub const TASK_SUFFIX: &str = ".task.json";

/// Suffix for worker response files: `<uuid>.response.json`
pub const WORKER_RESPONSE_SUFFIX: &str = ".response.json";

// =============================================================================
// Path helpers (shared by daemon IO and workers)
// =============================================================================

/// Build path to a worker's ready file.
#[must_use]
pub fn ready_path(agents_dir: &Path, uuid: &str) -> PathBuf {
    agents_dir.join(format!("{uuid}{READY_SUFFIX}"))
}

/// Build path to a worker's task file.
#[must_use]
pub fn task_path(agents_dir: &Path, uuid: &str) -> PathBuf {
    agents_dir.join(format!("{uuid}{TASK_SUFFIX}"))
}

/// Build path to a worker's response file.
#[must_use]
pub fn response_path(agents_dir: &Path, uuid: &str) -> PathBuf {
    agents_dir.join(format!("{uuid}{WORKER_RESPONSE_SUFFIX}"))
}
