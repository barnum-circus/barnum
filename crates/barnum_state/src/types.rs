//! State log entry types for NDJSON persistence.

use barnum_types::{LogTaskId, StepInputValue, StepName};
use serde::{Deserialize, Serialize};

/// A single entry in the state log.
///
/// The log is a sequence of these entries in NDJSON format.
/// The first entry **must** be `Config` (exactly once).
/// Subsequent entries are `TaskSubmitted` or `TaskCompleted`.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StateLogEntry {
    /// The run's configuration, recorded once at the start.
    Config(StateLogConfig),
    /// A task was submitted for execution.
    TaskSubmitted(TaskSubmitted),
    /// A task completed (success or failure).
    TaskCompleted(TaskCompleted),
}

/// Configuration snapshot stored in the state log.
///
/// Stored as raw JSON so `barnum_state` doesn't depend on `barnum_config`.
/// The caller deserializes to the concrete `Config` type.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StateLogConfig {
    /// The full resolved config as a JSON value.
    pub config: serde_json::Value,
}

/// Record of a task being submitted for execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSubmitted {
    /// Unique ID for this task instance.
    pub task_id: LogTaskId,
    /// Which step this task executes.
    pub step: StepName,
    /// The input value for this task.
    pub value: StepInputValue,
    /// Parent task waiting for this one to complete.
    pub parent_id: Option<LogTaskId>,
    /// How this task came to exist.
    pub origin: TaskOrigin,
}

/// How a task came to be created.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum TaskOrigin {
    /// From `--initial-state` (root task).
    Initial,
    /// Spawned by parent task's action output.
    Spawned,
    /// Retry of a failed task.
    Retry {
        /// The task this replaces.
        replaces: LogTaskId,
    },
    /// Finally hook for a completed task.
    Finally {
        /// The task whose finally hook this is.
        finally_for: LogTaskId,
    },
}

/// Record of a task completing.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskCompleted {
    /// The task that completed.
    pub task_id: LogTaskId,
    /// How it completed.
    pub outcome: TaskOutcome,
}

/// Outcome of a completed task.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum TaskOutcome {
    /// Task succeeded.
    Success(TaskSuccess),
    /// Task failed.
    Failed(TaskFailed),
}

/// Details of a successful task completion.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskSuccess {
    /// IDs of child tasks spawned by this task's output.
    pub spawned_task_ids: Vec<LogTaskId>,
    /// The (post-pre-hook) input value for scheduling the finally hook.
    ///
    /// Stored here so that on resume, `WaitingForChildren` tasks can
    /// reconstruct their `finally_data` without re-running the pre-hook.
    pub finally_value: StepInputValue,
}

/// Details of a task failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFailed {
    /// Why the task failed.
    pub reason: FailureReason,
    /// If the task was retried, the ID of the replacement task.
    /// `None` if retries were exhausted or disabled.
    pub retry_task_id: Option<LogTaskId>,
}

/// Why a task failed.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum FailureReason {
    /// Task exceeded its timeout.
    Timeout,
    /// Agent disappeared without responding.
    AgentLost,
    /// Agent returned an unparseable or invalid response.
    InvalidResponse {
        /// Human-readable description of what went wrong.
        message: String,
    },
}
