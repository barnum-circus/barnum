//! Shared constants for the agent pool protocol.

/// Directory containing agent subdirectories.
pub const AGENTS_DIR: &str = "agents";

/// Directory for file-based task submissions (sandboxed environments).
pub const PENDING_DIR: &str = "pending";

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
