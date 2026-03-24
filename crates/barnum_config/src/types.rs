//! Domain types for Barnum config.
//!
//! Re-exports types from `barnum_types` for use within this crate.

pub use barnum_types::{HookScript, LogTaskId, StepInputValue, StepName};

/// A task with its kind (step name) and value.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Task {
    /// The step name (serialized as "kind" for compatibility with agent responses).
    #[serde(rename = "kind")]
    pub step: StepName,
    /// The task payload.
    pub value: StepInputValue,
    /// Number of times this task has been retried (internal tracking, not serialized).
    #[serde(skip)]
    pub(crate) retries: u32,
}

impl Task {
    /// Create a new task with the given step name and value.
    #[must_use]
    pub fn new(step: impl Into<StepName>, value: StepInputValue) -> Self {
        Self {
            step: step.into(),
            value,
            retries: 0,
        }
    }
}
