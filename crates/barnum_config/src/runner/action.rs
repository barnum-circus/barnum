//! Unified action error type.

use std::fmt;

/// Error from action dispatch.
///
/// `TimedOut` is produced exclusively by `run_action` (Phase 1) when
/// `recv_timeout` fires. Actions return `Result<String, String>`, and
/// `run_action` wraps `Err(msg)` into `Failed(msg)`.
pub enum ActionError {
    /// Timeout fired (produced by `run_action`, not by actions themselves).
    #[expect(dead_code, reason = "constructed by run_action in Phase 1")]
    TimedOut,
    /// The action returned an error.
    Failed(String),
}

impl fmt::Display for ActionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TimedOut => write!(f, "action timed out"),
            Self::Failed(msg) => write!(f, "{msg}"),
        }
    }
}
