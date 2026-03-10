//! Structured response protocol for task results.
//!
//! The daemon returns structured JSON responses that distinguish between
//! success and various failure modes:
//!
//! ```json
//! {"kind": "Processed", "stdout": "...", "stderr": "..."}
//! {"kind": "NotProcessed", "reason": "timeout"}
//! {"kind": "NotProcessed", "reason": "stopped"}
//! ```

use serde::{Deserialize, Serialize};

/// Reason why task completion couldn't be confirmed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotProcessedReason {
    /// The pool is stopping.
    Stopped,
    /// Timed out waiting for confirmation.
    Timeout,
}

/// A structured response from task execution.
///
/// Uses an internally-tagged enum to make invalid states unrepresentable:
/// - `Processed` always has `stdout`, never has `reason`
/// - `NotProcessed` always has `reason`, never has `stdout`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Response {
    /// Task was fully processed by an agent.
    Processed {
        /// Standard output from the agent.
        stdout: String,
        /// Standard error from the agent (if any).
        #[serde(skip_serializing_if = "Option::is_none")]
        stderr: Option<String>,
    },
    /// Task completion status is unknown.
    ///
    /// The task may still be running, may have completed, or may have failed -
    /// we simply didn't receive confirmation. This can happen due to timeout,
    /// shutdown, or other interruptions. The caller should decide whether to
    /// retry based on the reason and idempotency of the task.
    NotProcessed {
        /// Why processing couldn't be confirmed.
        reason: NotProcessedReason,
    },
}

impl Response {
    /// Create a successful response with the agent's output.
    #[must_use]
    pub const fn processed(stdout: String) -> Self {
        Self::Processed {
            stdout,
            stderr: None,
        }
    }

    /// Create a response for when processing was not completed.
    #[must_use]
    pub const fn not_processed(reason: NotProcessedReason) -> Self {
        Self::NotProcessed { reason }
    }

    /// Returns the response kind for pattern matching.
    #[must_use]
    pub const fn kind(&self) -> ResponseKind {
        match self {
            Self::Processed { .. } => ResponseKind::Processed,
            Self::NotProcessed { .. } => ResponseKind::NotProcessed,
        }
    }
}

/// The kind of response from a task execution.
///
/// This is a convenience enum for matching without destructuring the full response.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResponseKind {
    /// Task was fully processed by an agent.
    Processed,
    /// Task completion status is unknown.
    NotProcessed,
}

#[cfg(test)]
#[expect(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn processed_serializes_correctly() {
        let response = Response::processed("hello world".to_string());
        let json = serde_json::to_string(&response).expect("serialize failed");
        // Keys are lowercase, values are UpperCamelCase
        assert!(json.contains(r#""kind":"Processed""#));
        assert!(json.contains(r#""stdout":"hello world""#));
        assert!(!json.contains("reason"));
    }

    #[test]
    fn not_processed_serializes_correctly() {
        let response = Response::not_processed(NotProcessedReason::Stopped);
        let json = serde_json::to_string(&response).expect("serialize failed");
        // Keys are lowercase, values are UpperCamelCase
        assert!(json.contains(r#""kind":"NotProcessed""#));
        assert!(json.contains(r#""reason":"stopped""#));
        assert!(!json.contains("stdout"));
    }

    #[test]
    fn roundtrip_processed() {
        let original = Response::processed("test output".to_string());
        let json = serde_json::to_string(&original).expect("serialize failed");
        let parsed: Response = serde_json::from_str(&json).expect("deserialize failed");
        assert_eq!(parsed.kind(), ResponseKind::Processed);
        assert!(matches!(parsed, Response::Processed { stdout, .. } if stdout == "test output"));
    }

    #[test]
    fn roundtrip_not_processed() {
        let original = Response::not_processed(NotProcessedReason::Timeout);
        let json = serde_json::to_string(&original).expect("serialize failed");
        let parsed: Response = serde_json::from_str(&json).expect("deserialize failed");
        assert_eq!(parsed.kind(), ResponseKind::NotProcessed);
        assert!(
            matches!(parsed, Response::NotProcessed { reason } if reason == NotProcessedReason::Timeout)
        );
    }
}
