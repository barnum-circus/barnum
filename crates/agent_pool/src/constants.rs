//! Shared constants for the agent pool protocol.

/// Directory containing agent subdirectories.
pub const AGENTS_DIR: &str = "agents";

/// Lock file for single-daemon enforcement.
pub const LOCK_FILE: &str = "daemon.lock";

/// Socket name for IPC (file path on Unix, named pipe on Windows).
pub const SOCKET_NAME: &str = "daemon.sock";

/// Stable filename for task input.
pub const TASK_FILE: &str = "task.json";

/// Stable filename for agent response.
pub const RESPONSE_FILE: &str = "response.json";

/// Stable filename for agent heartbeat.
pub const HEARTBEAT_FILE: &str = "heartbeat";
