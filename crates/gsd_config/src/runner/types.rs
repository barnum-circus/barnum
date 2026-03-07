//! Types for the task runner.

use agent_pool::Response;
use agent_pool_cli::AgentPoolCli;
use cli_invoker::Invoker;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::Path;

use crate::types::{LogTaskId, StepName};
use crate::value_schema::Task;

/// Input/output for post hooks.
///
/// Post hooks receive this JSON on stdin and must output (possibly modified)
/// JSON on stdout. The `next` array can be filtered, added to, or transformed.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum PostHookInput {
    /// The action completed successfully.
    Success {
        /// The input value (possibly modified by pre hook).
        input: serde_json::Value,
        /// The agent's output.
        output: serde_json::Value,
        /// Tasks spawned by this completion. Post hook can modify this.
        next: Vec<Task>,
    },
    /// The action timed out.
    Timeout {
        /// The input value (possibly modified by pre hook).
        input: serde_json::Value,
    },
    /// The action failed with an error.
    Error {
        /// The input value (possibly modified by pre hook).
        input: serde_json::Value,
        /// Error message.
        error: String,
    },
    /// The pre hook failed.
    PreHookError {
        /// The original input value (before pre hook).
        input: serde_json::Value,
        /// Error message from pre hook.
        error: String,
    },
}

/// Runner configuration.
pub struct RunnerConfig<'a> {
    /// Path to the `agent_pool` root directory.
    pub agent_pool_root: &'a Path,
    /// Working directory for command actions (typically the config file's directory).
    pub working_dir: &'a Path,
    /// Optional wake script to call before starting.
    pub wake_script: Option<&'a str>,
    /// Initial tasks to process (must not be empty).
    pub initial_tasks: Vec<Task>,
    /// Invoker for the `agent_pool` CLI.
    pub invoker: &'a Invoker<AgentPoolCli>,
}

/// The outcome of processing a task.
#[derive(Debug)]
pub struct TaskOutcome {
    /// The task that was processed.
    pub task: Task,
    /// What happened to the task.
    pub result: TaskResult,
}

/// Result of processing a single task.
#[derive(Debug)]
pub enum TaskResult {
    /// Task completed successfully, spawning new tasks.
    Completed {
        /// New tasks spawned by this task's completion.
        new_tasks: Vec<Task>,
    },
    /// Task was requeued for retry.
    Requeued {
        /// Reason for retry.
        reason: String,
        /// Current retry count.
        retry_count: u32,
    },
    /// Task was dropped (validation failed or retries exhausted).
    Dropped {
        /// Reason the task was dropped.
        reason: String,
    },
    /// Task was skipped (unknown step or validation failure).
    Skipped {
        /// Reason the task was skipped.
        reason: String,
    },
}

/// Internal task wrapper with lineage tracking.
pub(super) struct QueuedTask {
    pub task: Task,
    /// Unique ID for this task instance.
    pub id: LogTaskId,
    /// If this task descended from a task with `finally`, tracks that origin.
    pub origin_id: Option<LogTaskId>,
}

/// Result from an in-flight task submission.
pub(super) struct InFlightResult {
    pub task: Task,
    pub task_id: LogTaskId,
    pub origin_id: Option<LogTaskId>,
    pub step_name: StepName,
    /// The value passed to the action (possibly modified by pre hook).
    pub effective_value: serde_json::Value,
    pub result: SubmitResult,
    /// Post hook command to run after processing (if any).
    pub post_hook: Option<String>,
    /// Finally hook for this step (if any) - used when spawning children.
    pub finally_hook: Option<String>,
}

/// Result of submitting a task.
pub(super) enum SubmitResult {
    Pool(io::Result<Response>),
    Command(io::Result<String>),
    /// Pre hook failed before the action could run.
    PreHookError(String),
}
