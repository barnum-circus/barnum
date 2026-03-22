//! State log entry types for NDJSON persistence.

use barnum_types::{LogTaskId, StepInputValue, StepName};
use serde::{Deserialize, Serialize};

/// A single entry in the state log.
///
/// The log is a sequence of these entries in NDJSON format.
/// The first entry **must** be `Config` (exactly once).
/// Subsequent entries record task lifecycle events.
///
/// Seed tasks appear as top-level `TaskSubmitted` entries. All other tasks
/// (children, retries) are embedded inside [`TaskCompleted`] or [`FinallyRun`]
/// entries as part of the parent's outcome.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StateLogEntry {
    /// The run's configuration, recorded once at the start.
    Config(StateLogConfig),
    /// A seed task was submitted for execution.
    TaskSubmitted(TaskSubmitted),
    /// A task completed (success or failure).
    TaskCompleted(TaskCompleted),
    /// A finally hook ran for a parent whose children all completed.
    FinallyRun(FinallyRun),
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
    /// How this task came to exist.
    pub origin: TaskOrigin,
}

/// How a task came to be created.
///
/// Each variant carries only non-derivable information:
/// - `Seed` has no relationships.
/// - `Spawned` needs the parent explicitly.
/// - `Retry` references the replaced task; parent is derived from it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind")]
pub enum TaskOrigin {
    /// Seed task from initial input.
    Seed,
    /// Spawned by a parent task's action output or finally hook.
    ///
    /// `parent_id` is `Some` for children under a parent, `None` for
    /// children spawned by a root task's finally hook (no grandparent).
    Spawned {
        /// The parent task waiting for this one. `None` only for children
        /// spawned by a root-level finally hook.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        parent_id: Option<LogTaskId>,
    },
    /// Retry of a failed task. Parent inherited from the replaced task.
    Retry {
        /// The task this replaces.
        replaces: LogTaskId,
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
///
/// Each variant is self-contained. `Success` carries the finally value and
/// children (empty for leaf tasks). `Failed` carries the reason and an
/// optional retry task.
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
    /// The (post-pre-hook) input value for scheduling the finally hook.
    ///
    /// Stored so that on resume, the Engine can reconstruct `finally_value`
    /// for `WaitingForChildren` tasks without re-running the pre-hook.
    pub finally_value: StepInputValue,
    /// Child tasks spawned by this task's output. Empty for leaf tasks.
    pub children: Vec<TaskSubmitted>,
}

/// Details of a task failure.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TaskFailed {
    /// Why the task failed.
    pub reason: FailureReason,
    /// The retry replacement task, if retries remain.
    /// `None` if retries were exhausted or disabled.
    pub retry: Option<TaskSubmitted>,
}

/// Record of a finally hook executing for a parent task.
///
/// Produced when all of a parent's children complete and the parent's step
/// has a finally hook. The hook runs and may spawn new tasks. Presence of
/// this entry in the log means the finally ran — absence means it needs
/// re-dispatch on resume.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FinallyRun {
    /// The parent task whose finally hook ran.
    pub finally_for: LogTaskId,
    /// Child tasks spawned by the finally hook output.
    pub children: Vec<TaskSubmitted>,
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
