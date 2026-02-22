//! Structured response protocol for task results.
//!
//! The daemon returns structured JSON responses that distinguish between
//! success and various failure modes. Keys are lowercase, values `UpperCamelCase`:
//!
//! ```json
//! {"kind": "Processed", "stdout": "...", "stderr": "..."}
//! {"kind": "NotProcessed", "reason": "timeout"}
//! {"kind": "NotProcessed", "reason": "shutdown"}
//! ```

use serde::{Deserialize, Serialize};

/// The kind of response from a task execution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ResponseKind {
    /// Task was fully processed by an agent.
    Processed,
    /// Task was not processed (timeout, shutdown, etc.).
    NotProcessed,
}

/// Reason why a task was not processed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum NotProcessedReason {
    /// The daemon is shutting down.
    Shutdown,
    /// Task timed out waiting for an agent.
    Timeout,
}

/// A structured response from task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    /// The kind of response (values are `UpperCamelCase` like `Processed`).
    pub kind: ResponseKind,

    /// Standard output from the agent (present when processed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stdout: Option<String>,

    /// Standard error from the agent (present when processed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stderr: Option<String>,

    /// Reason for not processing (present when not processed).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<NotProcessedReason>,
}

impl Response {
    /// Create a successful response with the agent's output.
    #[must_use]
    pub const fn processed(stdout: String) -> Self {
        Self {
            kind: ResponseKind::Processed,
            stdout: Some(stdout),
            stderr: None,
            reason: None,
        }
    }

    /// Create a response for when processing was not completed.
    #[must_use]
    pub const fn not_processed(reason: NotProcessedReason) -> Self {
        Self {
            kind: ResponseKind::NotProcessed,
            stdout: None,
            stderr: None,
            reason: Some(reason),
        }
    }
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
        let response = Response::not_processed(NotProcessedReason::Shutdown);
        let json = serde_json::to_string(&response).expect("serialize failed");
        // Keys are lowercase, values are UpperCamelCase
        assert!(json.contains(r#""kind":"NotProcessed""#));
        assert!(json.contains(r#""reason":"shutdown""#));
        assert!(!json.contains("stdout"));
    }

    #[test]
    fn roundtrip_processed() {
        let original = Response::processed("test output".to_string());
        let json = serde_json::to_string(&original).expect("serialize failed");
        let parsed: Response = serde_json::from_str(&json).expect("deserialize failed");
        assert_eq!(parsed.kind, ResponseKind::Processed);
        assert_eq!(parsed.stdout, Some("test output".to_string()));
    }

    #[test]
    fn roundtrip_not_processed() {
        let original = Response::not_processed(NotProcessedReason::Timeout);
        let json = serde_json::to_string(&original).expect("serialize failed");
        let parsed: Response = serde_json::from_str(&json).expect("deserialize failed");
        assert_eq!(parsed.kind, ResponseKind::NotProcessed);
        assert_eq!(parsed.reason, Some(NotProcessedReason::Timeout));
    }
}
