//! Shared constants for the agent pool protocol.

/// Directory containing agent subdirectories.
pub const AGENTS_DIR: &str = "agents";

/// Lock file for single-daemon enforcement.
pub const LOCK_FILE: &str = "daemon.lock";

/// Socket name for IPC (file path on Unix, named pipe on Windows).
pub const SOCKET_NAME: &str = "daemon.sock";

/// File written by daemon when assigning work to an agent.
pub const NEXT_TASK_FILE: &str = "next_task";

/// File the agent renames `next_task` to when it starts processing.
/// This atomic rename prevents race conditions.
pub const IN_PROGRESS_FILE: &str = "in_progress";

/// File written by agent when work is complete.
pub const OUTPUT_FILE: &str = "output";
