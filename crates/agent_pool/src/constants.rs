//! Shared constants for the agent pool protocol.

/// Directory containing agent subdirectories.
pub const AGENTS_DIR: &str = "agents";

/// Lock file for single-daemon enforcement.
pub const LOCK_FILE: &str = "daemon.lock";

/// Socket name for IPC (file path on Unix, named pipe on Windows).
pub const SOCKET_NAME: &str = "daemon.sock";

/// File extension for task input files (e.g., `1.input`).
pub const INPUT_EXT: &str = "input";

/// File extension for task output files (e.g., `1.output`).
pub const OUTPUT_EXT: &str = "output";

// Legacy constants for backwards compatibility during transition
#[doc(hidden)]
pub const NEXT_TASK_FILE: &str = "next_task";
#[doc(hidden)]
pub const IN_PROGRESS_FILE: &str = "in_progress";
#[doc(hidden)]
pub const OUTPUT_FILE: &str = "output";
