//! Types for the task runner.

use std::io;
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};

use agent_pool::Response;
use agent_pool_cli::AgentPoolCli;
use cli_invoker::Invoker;
use serde::{Deserialize, Serialize};

use crate::types::{HookScript, LogTaskId, StepInputValue, StepName};
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
        /// The task's input value.
        input: StepInputValue,
        /// The agent's output.
        output: serde_json::Value,
        /// Tasks spawned by this completion. Post hook can modify this.
        next: Vec<Task>,
    },
    /// The action timed out.
    Timeout {
        /// The task's input value.
        input: StepInputValue,
    },
    /// The action failed with an error.
    Error {
        /// The task's input value.
        input: StepInputValue,
        /// Error message.
        error: String,
    },
    /// The pre hook failed.
    PreHookError {
        /// The original input value (before pre hook).
        input: StepInputValue,
        /// Error message from pre hook.
        error: String,
    },
}

/// Runner configuration (how to run, not what to run).
pub struct RunnerConfig<'a> {
    /// Path to the `agent_pool` root directory.
    pub agent_pool_root: &'a Path,
    /// Working directory for command actions (typically the config file's directory).
    pub working_dir: &'a Path,
    /// Optional wake script to call before starting.
    pub wake_script: Option<&'a str>,
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
}

/// Outcome of processing a task submission.
///
/// Separates spawned children (from successful execution) from retries (failed execution).
/// This distinction is crucial for finally hook tracking:
/// - Spawned children are "descendants" that the parent waits for
/// - Retries are continuations of the same logical task, not new descendants
pub(super) enum TaskOutcome {
    /// Task succeeded, may have spawned children.
    Success {
        spawned: Vec<Task>,
        finally_value: StepInputValue,
    },
    /// Task failed, should be retried.
    Retry(Task),
    /// Task failed permanently (max retries exceeded or retry disabled).
    Dropped,
}

/// Entry in the unified task state map.
pub(super) struct TaskEntry {
    /// The step this task is executing.
    pub step: StepName,
    /// Parent task waiting for this task to complete.
    pub parent_id: Option<LogTaskId>,
    /// **"Am I a finally task?"** (this task's type)
    ///
    /// - `None` = Step task (run pre-hook, then action)
    /// - `Some` = Finally task with this script (no pre-hook, just run script)
    ///
    /// The script is looked up once when the finally is scheduled, not again at dispatch.
    ///
    /// **Not to be confused with `finally_data` in `WaitingForChildren`:**
    /// - `finally_script`: "Am I a finally task?" (this task's type)
    /// - `finally_data`:   "Do I have a finally hook to run after my children?" (step's config)
    #[expect(dead_code)] // Used in Branch 4 (finally/04-completion-flow)
    pub finally_script: Option<HookScript>,
    /// Current state of this task.
    pub state: TaskState,
    /// Number of retries remaining for this task.
    #[expect(dead_code)] // Used in Branch 4 (finally/04-completion-flow)
    pub retries_remaining: u32,
}

/// State of a task in the runner.
pub(super) enum TaskState {
    /// Task waiting to be dispatched (queued due to concurrency limit).
    Pending {
        /// The step input value. For Step tasks, may be transformed by pre-hook.
        /// For Finally tasks, comes from parent (already through pre-hook).
        value: StepInputValue,
    },
    /// Task currently executing in a worker thread.
    InFlight(InFlight),
    /// Task completed its action, waiting for children to complete.
    WaitingForChildren {
        /// Number of children still pending.
        pending_children_count: NonZeroU16,
        /// **"Does this step have a finally hook to run after children?"** (step's config)
        ///
        /// Hook + value to schedule finally when all children complete.
        /// - `Some` for Step tasks whose step config has a finally hook
        /// - `None` for Finally tasks (no "finally of finally")
        ///
        /// The hook is looked up once when entering this state, not again when scheduling.
        finally_data: Option<(HookScript, StepInputValue)>,
    },
}

/// Zero-sized marker that a task is currently executing.
///
/// Only created when spawning a worker thread, enforcing that
/// `InFlight` state means the task is actually running.
pub(super) struct InFlight(());

impl InFlight {
    /// Create an `InFlight` marker.
    ///
    /// # Safety (invariant)
    ///
    /// Only call this immediately after spawning a worker thread for the task.
    pub(super) const fn new() -> Self {
        InFlight(())
    }
}

/// Identity of a task being processed.
#[derive(Clone)]
pub(super) struct TaskIdentity {
    pub task: Task,
    pub task_id: LogTaskId,
}

/// Result of task execution, returned from dispatch threads.
pub(super) struct InFlightResult {
    pub identity: TaskIdentity,
    pub result: SubmitResult,
}

/// Result of task submission. Value only exists when pre-hook succeeded.
pub(super) enum SubmitResult {
    Pool {
        value: StepInputValue,
        response: io::Result<Response>,
    },
    Command {
        value: StepInputValue,
        output: io::Result<String>,
    },
    PreHookError(String),
}
