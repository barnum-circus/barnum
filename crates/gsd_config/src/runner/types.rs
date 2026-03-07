//! Types for the task runner.

use agent_pool::Response;
use agent_pool_cli::AgentPoolCli;
use cli_invoker::Invoker;
use serde::{Deserialize, Serialize};
use std::io;
use std::path::{Path, PathBuf};

use crate::types::{HookScript, LogTaskId, StepName};
use crate::value_schema::Task;

/// Connection details for the agent pool.
pub(super) struct PoolConnection {
    pub root: PathBuf,
    pub working_dir: PathBuf,
    pub invoker: Invoker<AgentPoolCli>,
}

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

/// Result of task processing.
#[derive(Debug)]
pub(super) enum TaskResult {
    /// Task completed successfully.
    Completed,
    /// Task will be retried.
    Requeued,
    /// Task was dropped after exhausting retries.
    Dropped,
    /// Task was skipped (step not found).
    Skipped,
}

/// Task queued for execution.
pub(super) struct QueuedTask {
    pub task: Task,
    pub id: LogTaskId,
    pub origin_id: Option<LogTaskId>,
}

/// Identity of a task being processed.
pub(super) struct TaskIdentity {
    pub task: Task,
    pub task_id: LogTaskId,
    pub origin_id: Option<LogTaskId>,
    pub step_name: StepName,
}

/// Result of task execution, returned from dispatch threads.
pub(super) struct InFlightResult {
    pub identity: TaskIdentity,
    pub effective_value: serde_json::Value,
    pub result: SubmitResult,
    pub post_hook: Option<HookScript>,
    pub finally_hook: Option<HookScript>,
}

pub(super) enum SubmitResult {
    Pool(io::Result<Response>),
    Command(io::Result<String>),
    PreHookError(String),
}
