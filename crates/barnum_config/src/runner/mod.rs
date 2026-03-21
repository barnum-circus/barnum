//! Task queue runner for Barnum.
//!
//! Executes tasks through `troupe`, validating transitions and handling timeouts.

mod dispatch;
mod hooks;
mod response;
mod shell;
mod submit;

use std::collections::{BTreeMap, HashMap};
use std::io::{self, Write as _};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;

use barnum_state::{
    FailureReason, ReconstructedState, StateLogConfig, StateLogEntry, TaskCompleted, TaskFailed,
    TaskOrigin, TaskSubmitted,
};
use cli_invoker::Invoker;
use tracing::{error, info, warn};
use troupe_cli::TroupeCli;

use crate::docs::generate_step_docs;
use crate::resolved::{Action, Config, Step};
use crate::types::{HookScript, LogTaskId, StepInputValue, StepName};
use crate::value_schema::{CompiledSchemas, Task};

use dispatch::{
    InFlightResult, TaskContext, TaskIdentity, dispatch_command_task, dispatch_finally_task,
    dispatch_pool_task,
};
use hooks::{call_wake_script, run_post_hook};
use response::{FailureKind, ProcessedSubmit, TaskOutcome, process_retry, process_submit_result};

pub use hooks::PostHookInput;

// ==================== Public API ====================

/// Runner configuration (how to run, not what to run).
pub struct RunnerConfig<'a> {
    /// Path to the `troupe` root directory.
    pub troupe_root: &'a Path,
    /// Working directory for command actions (typically the config file's directory).
    pub working_dir: &'a Path,
    /// Optional wake script to call before starting.
    pub wake_script: Option<&'a str>,
    /// Invoker for the `troupe` CLI.
    pub invoker: &'a Invoker<TroupeCli>,
    /// Path for state log (NDJSON file for persistence/resume).
    pub state_log_path: &'a Path,
}

// ==================== Internal Types ====================

/// Connection details for the agent pool.
struct PoolConnection {
    root: PathBuf,
    working_dir: PathBuf,
    invoker: Invoker<TroupeCli>,
}

/// Result of task processing (for iterator).
#[derive(Debug)]
enum TaskResult {
    /// Task completed successfully.
    Completed,
    /// Task will be retried.
    Requeued,
    /// Task was dropped after exhausting retries.
    Dropped,
}

/// Entry in the unified task state map.
struct TaskEntry {
    /// The step this task is executing.
    step: StepName,
    /// Parent task waiting for this task to complete.
    parent_id: Option<LogTaskId>,
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
    finally_script: Option<HookScript>,
    /// Current state of this task.
    state: TaskState,
    /// Number of retries remaining for this task.
    // TODO: Use this for finally task retries (currently uses Task.retries like other tasks)
    #[expect(dead_code)]
    retries_remaining: u32,
}

/// State of a task in the runner.
enum TaskState {
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
struct InFlight(());

impl InFlight {
    /// Create an `InFlight` marker.
    ///
    /// # Safety (invariant)
    ///
    /// Only call this immediately after spawning a worker thread for the task.
    const fn new() -> Self {
        InFlight(())
    }
}

/// Default maximum concurrent task submissions.
///
/// Limits parallel submissions to avoid exhausting inotify instances.
/// Linux defaults to `max_user_instances=128`.
const DEFAULT_MAX_CONCURRENCY: usize = 20;

/// Internal task queue runner.
///
/// Tasks are submitted concurrently, and results are yielded as they complete.
struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    /// All task state in one place. Tasks not in this map are fully done.
    /// `BTreeMap` ordering by key = FIFO dispatch order (task IDs are monotonic).
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    pool: PoolConnection,
    max_concurrency: usize,
    /// Cached count of `InFlight` tasks (for concurrency limiting).
    in_flight: usize,
    tx: mpsc::Sender<InFlightResult>,
    rx: mpsc::Receiver<InFlightResult>,
    /// Counter for assigning unique task IDs.
    next_task_id: u32,
    /// State log writer for persistence/resume.
    state_log: io::BufWriter<std::fs::File>,
}

impl<'a> TaskRunner<'a> {
    fn new(
        config: &'a Config,
        schemas: &'a CompiledSchemas,
        runner_config: &RunnerConfig<'a>,
        initial_tasks: Vec<Task>,
    ) -> io::Result<Self> {
        if let Some(script) = runner_config.wake_script {
            call_wake_script(script)?;
        }

        // Pool existence/readiness is checked by submit_via_cli on first task submission
        let max_concurrency = config.max_concurrency.unwrap_or(DEFAULT_MAX_CONCURRENCY);

        info!(
            tasks = initial_tasks.len(),
            pool_root = %runner_config.troupe_root.display(),
            invoker = %runner_config.invoker.description(),
            max_concurrency,
            "starting task queue"
        );

        let (tx, rx) = mpsc::channel();

        let pool = PoolConnection {
            root: runner_config.troupe_root.to_path_buf(),
            working_dir: runner_config.working_dir.to_path_buf(),
            invoker: Clone::clone(runner_config.invoker),
        };

        // Open state log file
        let state_log = {
            let file = std::fs::File::create(runner_config.state_log_path)?;
            io::BufWriter::new(file)
        };

        info!(state_log = %runner_config.state_log_path.display(), "state log");

        let mut runner = Self {
            config,
            schemas,
            step_map: config.step_map(),
            tasks: BTreeMap::new(),
            pool,
            max_concurrency,
            in_flight: 0,
            tx,
            rx,
            next_task_id: 0,
            state_log,
        };

        // Write config entry to state log
        let config_json = serde_json::to_value(config)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        runner.write_log(&StateLogEntry::Config(StateLogConfig {
            config: config_json,
        }));

        for task in initial_tasks {
            // Validate step exists
            if !runner.step_map.contains_key(&task.step) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("[E019] unknown step '{}' in initial tasks", task.step),
                ));
            }

            // Validate value against step's schema
            if let Err(e) = schemas.validate(&task.step, &task.value) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!("[E020] initial task validation failed: {e}"),
                ));
            }

            let task_id = LogTaskId(runner.next_task_id);
            runner.write_log(&StateLogEntry::TaskSubmitted(TaskSubmitted {
                task_id,
                step: task.step.clone(),
                value: task.value.clone(),
                parent_id: None,
                origin: TaskOrigin::Initial,
            }));
            runner.queue_task(task, None, None);
        }

        Ok(runner)
    }

    /// Create a `TaskRunner` from reconstructed state (for resume).
    ///
    /// The state log should already contain copied entries from the old log.
    fn new_resumed(
        config: &'a Config,
        schemas: &'a CompiledSchemas,
        runner_config: &RunnerConfig<'a>,
        state: ReconstructedState,
        state_log: io::BufWriter<std::fs::File>,
    ) -> io::Result<Self> {
        if let Some(script) = runner_config.wake_script {
            call_wake_script(script)?;
        }

        let max_concurrency = config.max_concurrency.unwrap_or(DEFAULT_MAX_CONCURRENCY);

        info!(
            pending = state.pending_tasks.len(),
            waiting = state.waiting_tasks.len(),
            next_task_id = state.next_task_id,
            pool_root = %runner_config.troupe_root.display(),
            max_concurrency,
            "resuming task queue"
        );

        let (tx, rx) = mpsc::channel();

        let pool = PoolConnection {
            root: runner_config.troupe_root.to_path_buf(),
            working_dir: runner_config.working_dir.to_path_buf(),
            invoker: Clone::clone(runner_config.invoker),
        };

        let mut runner = Self {
            config,
            schemas,
            step_map: config.step_map(),
            tasks: BTreeMap::new(),
            pool,
            max_concurrency,
            in_flight: 0,
            tx,
            rx,
            next_task_id: state.next_task_id,
            state_log,
        };

        runner.load_reconstructed_state(state);
        Ok(runner)
    }

    /// Load pre-existing task state from a reconstructed log.
    ///
    /// Waiting tasks become `WaitingForChildren` entries in the task map.
    /// Pending tasks become `Pending` entries (will be dispatched on next iteration).
    fn load_reconstructed_state(&mut self, state: ReconstructedState) {
        // Load waiting tasks (completed, have alive children)
        for waiting in state.waiting_tasks {
            let finally_data = self
                .step_map
                .get(&waiting.step)
                .and_then(|s| s.finally_hook.clone())
                .map(|hook| (hook, waiting.finally_value));

            self.tasks.insert(
                waiting.task_id,
                TaskEntry {
                    step: waiting.step,
                    parent_id: waiting.parent_id,
                    finally_script: None,
                    state: TaskState::WaitingForChildren {
                        pending_children_count: waiting.pending_children_count,
                        finally_data,
                    },
                    retries_remaining: 0,
                },
            );
        }

        // Load pending tasks (need dispatch)
        for pending in state.pending_tasks {
            let retries_remaining = self
                .step_map
                .get(&pending.step)
                .map_or(0, |s| s.options.max_retries);

            // Finally tasks carry their hook script
            let finally_script = if matches!(pending.origin, TaskOrigin::Finally { .. }) {
                self.step_map
                    .get(&pending.step)
                    .and_then(|s| s.finally_hook.clone())
            } else {
                None
            };

            self.tasks.insert(
                pending.task_id,
                TaskEntry {
                    step: pending.step,
                    parent_id: pending.parent_id,
                    finally_script,
                    state: TaskState::Pending {
                        value: pending.value,
                    },
                    retries_remaining,
                },
            );
        }
    }

    fn pending(&self) -> usize {
        self.tasks
            .values()
            .filter(|e| matches!(e.state, TaskState::Pending { .. }))
            .count()
    }

    /// Allocate the next task ID.
    #[expect(clippy::missing_const_for_fn)] // &mut self can't be const
    fn next_task_id(&mut self) -> LogTaskId {
        let id = LogTaskId(self.next_task_id);
        self.next_task_id += 1;
        id
    }

    // ==================== State Log ====================

    /// Write an entry to the state log (no-op if logging is disabled).
    fn write_log(&mut self, entry: &StateLogEntry) {
        if let Err(e) = barnum_state::write_entry(&mut self.state_log, entry) {
            error!(error = %e, "failed to write state log entry");
        }
    }

    /// Map the runner's `FailureKind` to the state log's `FailureReason`.
    fn map_failure(kind: FailureKind) -> FailureReason {
        match kind {
            FailureKind::Timeout => FailureReason::Timeout,
            FailureKind::InvalidResponse => FailureReason::InvalidResponse {
                message: "invalid response".to_string(),
            },
            FailureKind::SubmitError => FailureReason::AgentLost,
        }
    }

    // ==================== State Transitions ====================

    /// Add a new task - dispatch immediately if under concurrency, otherwise queue as Pending.
    ///
    /// If `finally_script` is `Some`, this is a finally task (retry or initial).
    fn queue_task(
        &mut self,
        task: Task,
        parent_id: Option<LogTaskId>,
        finally_script: Option<HookScript>,
    ) {
        let id = self.next_task_id();
        let retries_remaining = self
            .step_map
            .get(&task.step)
            .map_or(0, |s| s.options.max_retries);

        if self.in_flight < self.max_concurrency {
            // Create entry in InFlight state and dispatch
            let prev = self.tasks.insert(
                id,
                TaskEntry {
                    step: task.step.clone(),
                    parent_id,
                    finally_script,
                    state: TaskState::InFlight(InFlight::new()),
                    retries_remaining,
                },
            );
            assert!(prev.is_none(), "task_id collision: {id:?} already in map");
            self.in_flight += 1;
            self.dispatch(id, task);
        } else {
            // Queue as Pending
            let prev = self.tasks.insert(
                id,
                TaskEntry {
                    step: task.step,
                    parent_id,
                    finally_script,
                    state: TaskState::Pending { value: task.value },
                    retries_remaining,
                },
            );
            assert!(prev.is_none(), "task_id collision: {id:?} already in map");
        }
    }

    /// Dispatch a task to a worker thread.
    ///
    /// Precondition: The task must already be in `InFlight` state in the map
    /// (set by `take_next_pending`).
    #[expect(clippy::expect_used)] // Invariants
    fn dispatch(&self, task_id: LogTaskId, task: Task) {
        let entry = self.tasks.get(&task_id).expect("[P014] task must exist");
        let tx = self.tx.clone();

        // Finally tasks: run the finally script directly (no pre-hook)
        if let Some(script) = &entry.finally_script {
            let script = script.clone();
            let identity = TaskIdentity { task, task_id };
            let working_dir = self.pool.working_dir.clone();

            info!(step = %identity.task.step, "dispatching finally task");

            thread::spawn(move || {
                dispatch_finally_task(identity, &script, &working_dir, &tx);
            });
            return;
        }

        // Regular tasks: dispatch based on step action
        let step = self.step_map.get(&task.step).expect("[P015] unknown step");

        let identity = TaskIdentity { task, task_id };
        let ctx = TaskContext {
            identity,
            pre_hook: step.pre.clone(),
        };

        match &step.action {
            Action::Pool { .. } => {
                let timeout = step.options.timeout;
                let docs = generate_step_docs(step, self.config);
                let pool_root = self.pool.root.clone();
                let working_dir = self.pool.working_dir.clone();
                let invoker = self.pool.invoker.clone();

                info!(step = %ctx.identity.task.step, "submitting task to pool");

                thread::spawn(move || {
                    dispatch_pool_task(
                        ctx,
                        &docs,
                        timeout,
                        &pool_root,
                        &working_dir,
                        &invoker,
                        &tx,
                    );
                });
            }
            Action::Command { script } => {
                let script = script.clone();
                let working_dir = self.pool.working_dir.clone();

                info!(step = %ctx.identity.task.step, script = %script, "executing command");

                thread::spawn(move || {
                    dispatch_command_task(ctx, &script, &working_dir, &tx);
                });
            }
        }
    }

    /// Dispatch pending tasks up to max concurrency.
    fn dispatch_all_pending(&mut self) {
        while self.in_flight < self.max_concurrency {
            let Some((task_id, task)) = self.take_next_pending() else {
                break;
            };
            self.dispatch(task_id, task);
        }
    }

    /// Find and extract the next pending task, transitioning it to `InFlight`.
    ///
    /// Returns `(task_id, task)` if a pending task was found.
    /// The task is transitioned to `InFlight` state in the map.
    fn take_next_pending(&mut self) -> Option<(LogTaskId, Task)> {
        let result = self.tasks.iter_mut().find_map(|(id, entry)| {
            if let TaskState::Pending { value } = &mut entry.state {
                let value = std::mem::replace(value, StepInputValue(serde_json::Value::Null));
                let task = Task::new(entry.step.as_str(), value);
                entry.state = TaskState::InFlight(InFlight::new());
                Some((*id, task))
            } else {
                None
            }
        });

        if result.is_some() {
            self.in_flight += 1;
        }
        result
    }

    /// Remove an `InFlight` task (for retry - don't notify parent).
    #[expect(clippy::expect_used)] // Invariant: task must exist
    fn transition_to_done(&mut self, task_id: LogTaskId) -> Option<LogTaskId> {
        let entry = self.tasks.remove(&task_id).expect("task must exist");
        assert!(matches!(entry.state, TaskState::InFlight(_)));
        self.in_flight -= 1;
        entry.parent_id
    }

    /// Look up the finally hook for a task's step, if any.
    /// Returns None for Finally tasks (no "finally of finally").
    fn lookup_finally_hook(&self, entry: &TaskEntry) -> Option<HookScript> {
        if entry.finally_script.is_some() {
            return None; // No "finally of finally"
        }
        self.config
            .steps
            .iter()
            .find(|s| s.name == entry.step)
            .and_then(|s| s.finally_hook.clone())
    }

    /// Schedule a finally task as a sibling of the given task.
    ///
    /// The finally task becomes a child of the original task's parent.
    /// Does NOT remove `task_id` - caller must do that.
    #[expect(clippy::expect_used)] // Invariant: task must exist
    fn schedule_finally(&mut self, task_id: LogTaskId, hook: HookScript, value: StepInputValue) {
        let entry = self.tasks.get(&task_id).expect("[P018] task must exist");
        let parent_id = entry.parent_id;
        let step = entry.step.clone();

        // Increment parent's pending count (finally becomes another child)
        if let Some(parent_id) = parent_id {
            self.increment_pending_children(parent_id);
        }

        // Create the finally task
        let id = self.next_task_id();
        let retries_remaining = self
            .step_map
            .get(&step)
            .map_or(0, |s| s.options.max_retries);

        // Log the finally task submission
        self.write_log(&StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: id,
            step: step.clone(),
            value: value.clone(),
            parent_id,
            origin: TaskOrigin::Finally {
                finally_for: task_id,
            },
        }));

        let finally_entry = TaskEntry {
            step,
            parent_id,
            finally_script: Some(hook),
            state: TaskState::Pending { value },
            retries_remaining,
        };
        self.tasks.insert(id, finally_entry);
    }

    /// Increment a task's `pending_children_count`.
    #[expect(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // Invariants
    fn increment_pending_children(&mut self, task_id: LogTaskId) {
        let entry = self
            .tasks
            .get_mut(&task_id)
            .expect("[P019] task must exist");
        let TaskState::WaitingForChildren {
            pending_children_count,
            ..
        } = &mut entry.state
        else {
            panic!("[P020] task not in WaitingForChildren state");
        };
        *pending_children_count = NonZeroU16::new(pending_children_count.get() + 1).unwrap();
    }

    /// Remove task and decrement parent's count.
    fn remove_and_notify_parent(&mut self, task_id: LogTaskId) {
        #[expect(clippy::expect_used)] // Invariant: task must exist
        let entry = self.tasks.remove(&task_id).expect("[P021] task must exist");
        if let Some(parent_id) = entry.parent_id {
            self.decrement_pending_children(parent_id);
        }
    }

    // ==================== Key Operations ====================

    /// Decrement a task's `pending_children_count`.
    /// When count hits zero: schedule finally (if any), then remove.
    #[expect(clippy::expect_used, clippy::panic, clippy::unwrap_used)] // Invariants
    fn decrement_pending_children(&mut self, task_id: LogTaskId) {
        let (hit_zero, finally_data) = {
            let entry = self
                .tasks
                .get_mut(&task_id)
                .expect("[P022] task must exist");
            let TaskState::WaitingForChildren {
                pending_children_count,
                finally_data,
            } = &mut entry.state
            else {
                panic!("[P023] task not in WaitingForChildren state");
            };

            let new_count = pending_children_count.get() - 1;
            if new_count == 0 {
                (true, finally_data.take())
            } else {
                *pending_children_count = NonZeroU16::new(new_count).unwrap();
                (false, None)
            }
        };

        if hit_zero {
            // Schedule finally as sibling (if any), then remove task
            if let Some((hook, value)) = finally_data {
                self.schedule_finally(task_id, hook, value);
            }
            self.remove_and_notify_parent(task_id);
        }
    }

    /// Handle task success.
    ///
    /// If task has no children:
    ///   - Schedule finally as sibling (if any)
    ///   - Remove task, notify parent
    ///
    /// If task has children:
    ///   - Transition to `WaitingForChildren` with `finally_data`
    ///   - Queue children
    #[expect(
        clippy::unwrap_used,
        clippy::cast_possible_truncation,
        clippy::expect_used
    )]
    fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
        self.in_flight -= 1;

        let entry = self.tasks.get(&task_id).expect("[P024] task must exist");
        let finally_hook = self.lookup_finally_hook(entry);

        if spawned.is_empty() {
            // Log completion with no children
            self.write_log(&StateLogEntry::TaskCompleted(TaskCompleted {
                task_id,
                outcome: barnum_state::TaskOutcome::Success(barnum_state::TaskSuccess {
                    spawned_task_ids: vec![],
                    finally_value: value.clone(),
                }),
            }));

            // No children - schedule finally (if any) as sibling, then remove
            if let Some(hook) = finally_hook {
                self.schedule_finally(task_id, hook, value);
            }
            self.remove_and_notify_parent(task_id);
        } else {
            // Compute child IDs before queuing (IDs are monotonically assigned)
            let first_child_id = self.next_task_id;
            let spawned_task_ids: Vec<LogTaskId> = (0..spawned.len())
                .map(|i| LogTaskId(first_child_id + i as u32))
                .collect();

            // Log completion with spawned child IDs
            self.write_log(&StateLogEntry::TaskCompleted(TaskCompleted {
                task_id,
                outcome: barnum_state::TaskOutcome::Success(barnum_state::TaskSuccess {
                    spawned_task_ids,
                    finally_value: value.clone(),
                }),
            }));

            // Has children - wait for them, storing finally_data
            let count = NonZeroU16::new(spawned.len() as u16).unwrap();
            let finally_data = finally_hook.map(|hook| (hook, value));

            let entry = self
                .tasks
                .get_mut(&task_id)
                .expect("[P025] task must exist");
            entry.state = TaskState::WaitingForChildren {
                pending_children_count: count,
                finally_data,
            };
            for child in spawned {
                // Log each spawned child
                let child_id = LogTaskId(self.next_task_id);
                self.write_log(&StateLogEntry::TaskSubmitted(TaskSubmitted {
                    task_id: child_id,
                    step: child.step.clone(),
                    value: child.value.clone(),
                    parent_id: Some(task_id),
                    origin: TaskOrigin::Spawned,
                }));
                self.queue_task(child, Some(task_id), None);
            }
        }
    }

    /// Handle task failure (with optional retry).
    #[expect(clippy::expect_used)] // Invariant: task must exist
    fn task_failed(&mut self, task_id: LogTaskId, retry: Option<Task>, failure_kind: FailureKind) {
        let entry = self.tasks.get(&task_id).expect("[P026] task must exist");
        let parent_id = entry.parent_id;
        let finally_script = entry.finally_script.clone();

        if let Some(retry_task) = retry {
            // Compute retry task ID before logging
            let retry_task_id = LogTaskId(self.next_task_id);

            // Log failure with retry
            self.write_log(&StateLogEntry::TaskCompleted(TaskCompleted {
                task_id,
                outcome: barnum_state::TaskOutcome::Failed(TaskFailed {
                    reason: Self::map_failure(failure_kind),
                    retry_task_id: Some(retry_task_id),
                }),
            }));

            // Log the retry task submission
            self.write_log(&StateLogEntry::TaskSubmitted(TaskSubmitted {
                task_id: retry_task_id,
                step: retry_task.step.clone(),
                value: retry_task.value.clone(),
                parent_id,
                origin: TaskOrigin::Retry { replaces: task_id },
            }));

            self.queue_task(retry_task, parent_id, finally_script);
            self.transition_to_done(task_id); // Don't notify - retry takes over
        } else {
            // Log permanent failure
            self.write_log(&StateLogEntry::TaskCompleted(TaskCompleted {
                task_id,
                outcome: barnum_state::TaskOutcome::Failed(TaskFailed {
                    reason: Self::map_failure(failure_kind),
                    retry_task_id: None,
                }),
            }));

            // Permanent failure - remove and notify parent
            let entry = self.tasks.remove(&task_id).expect("[P027] task must exist");
            if matches!(entry.state, TaskState::InFlight(_)) {
                self.in_flight -= 1;
            }
            if let Some(parent_id) = entry.parent_id {
                self.decrement_pending_children(parent_id);
            }
        }
    }

    fn process_result(&mut self, inflight: InFlightResult) -> TaskResult {
        let InFlightResult { identity, result } = inflight;

        let TaskIdentity { task, task_id } = identity;

        #[expect(clippy::expect_used)] // Invariant: all queued tasks are validated at entry points
        let step = self.step_map.get(&task.step).expect(
            "[P015] BUG: task step must exist - all queued tasks should be validated at entry points",
        );

        let ProcessedSubmit {
            outcome,
            post_input,
        } = process_submit_result(result, &task, step, self.schemas);

        // Post hook can modify the outcome (e.g., filter spawned tasks)
        let outcome = if let Some(hook) = &step.post {
            match run_post_hook(hook, &post_input, &self.pool.working_dir) {
                Ok(modified) => match outcome {
                    TaskOutcome::Success { finally_value, .. } => {
                        let tasks = extract_next_tasks(&modified);
                        TaskOutcome::Success {
                            spawned: tasks,
                            finally_value,
                        }
                    }
                    other => other,
                },
                Err(e) => {
                    warn!(step = %task.step, error = %e, "post hook failed");
                    process_retry(&task, &step.options, FailureKind::SubmitError)
                }
            }
        } else {
            outcome
        };

        match outcome {
            TaskOutcome::Success {
                spawned,
                finally_value,
            } => {
                self.task_succeeded(task_id, spawned, finally_value);
                TaskResult::Completed
            }

            TaskOutcome::Retry(retry_task, failure_kind) => {
                self.task_failed(task_id, Some(retry_task), failure_kind);
                TaskResult::Requeued
            }

            TaskOutcome::Dropped(failure_kind) => {
                self.task_failed(task_id, None, failure_kind);
                TaskResult::Dropped
            }
        }
    }
}

impl Iterator for TaskRunner<'_> {
    type Item = TaskResult;

    /// Get the next completed task outcome.
    ///
    /// Submits pending tasks concurrently and returns results as they complete.
    /// Returns `None` when all tasks are done (nothing pending, nothing in flight).
    fn next(&mut self) -> Option<Self::Item> {
        self.dispatch_all_pending();

        if self.in_flight == 0 {
            return None;
        }

        let result = self.rx.recv().ok()?;
        // Note: in_flight is decremented inside process_result when task transitions out of InFlight

        Some(self.process_result(result))
    }
}

/// Extract next tasks from a post hook result.
fn extract_next_tasks(input: &PostHookInput) -> Vec<Task> {
    match input {
        PostHookInput::Success { next, .. } => next.clone(),
        PostHookInput::Timeout { .. }
        | PostHookInput::Error { .. }
        | PostHookInput::PreHookError { .. } => vec![],
    }
}

/// Run the task queue to completion.
///
/// # Errors
///
/// Returns an error if the wake script fails or I/O errors occur.
pub fn run(
    config: &Config,
    schemas: &CompiledSchemas,
    runner_config: &RunnerConfig<'_>,
    initial_tasks: Vec<Task>,
) -> io::Result<()> {
    let mut runner = TaskRunner::new(config, schemas, runner_config, initial_tasks)?;
    run_to_completion(&mut runner)
}

/// Resume a run from a state log file.
///
/// Reads the old log, reconstructs state, optionally copies entries to a new
/// log, and continues executing pending tasks.
///
/// # Errors
///
/// Returns an error if the log is malformed, config deserialization fails,
/// or any I/O error occurs.
pub fn resume(old_log_path: &Path, runner_config: &RunnerConfig<'_>) -> io::Result<()> {
    // 1. Read and reconstruct state from old log
    let file = std::fs::File::open(old_log_path)?;
    let entries = barnum_state::read_entries(file);
    let (config_json, state) = barnum_state::reconstruct(entries)
        .map_err(|e| io::Error::other(format!("[E070] failed to reconstruct state: {e}")))?;

    // 2. Deserialize config from the log's stored config
    let config: Config = serde_json::from_value(config_json).map_err(|e| {
        io::Error::other(format!("[E071] failed to deserialize config from log: {e}"))
    })?;
    let schemas = CompiledSchemas::compile(&config)?;

    // 3. Open new state log and copy old entries
    info!(state_log = %runner_config.state_log_path.display(), "state log");
    let state_log = {
        let mut writer = io::BufWriter::new(std::fs::File::create(runner_config.state_log_path)?);
        let old_content = std::fs::read(old_log_path)?;
        writer.write_all(&old_content)?;
        writer.flush()?;
        writer
    };

    // 4. Create runner with reconstructed state and continue
    let mut runner = TaskRunner::new_resumed(&config, &schemas, runner_config, state, state_log)?;
    run_to_completion(&mut runner)
}

/// Drive a `TaskRunner` to completion.
fn run_to_completion(runner: &mut TaskRunner<'_>) -> io::Result<()> {
    let mut completed_count = 0u32;
    let mut dropped_count = 0u32;

    while let Some(result) = runner.next() {
        completed_count += 1;
        if matches!(result, TaskResult::Dropped) {
            dropped_count += 1;
        }

        let remaining = runner.pending();
        info!(
            "{} {} completed, {} {} remaining",
            completed_count,
            if completed_count == 1 {
                "task"
            } else {
                "tasks"
            },
            remaining,
            if remaining == 1 { "task" } else { "tasks" }
        );
    }

    if dropped_count > 0 {
        error!(dropped_count, "task queue completed with dropped tasks");
        return Err(io::Error::other(format!(
            "[E018] {dropped_count} task(s) were dropped (retries exhausted)"
        )));
    }
    info!(total = completed_count, "task queue complete");
    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::submit::build_agent_payload;
    use crate::types::StepName;

    #[test]
    fn build_payload_includes_task_and_docs() {
        let step_name = StepName::new("Test");
        let value = serde_json::json!({"x": 1});
        let docs = "# Test Step";

        let payload = build_agent_payload(&step_name, &value, docs, Some(60));
        let parsed: serde_json::Value = serde_json::from_str(&payload).unwrap();

        assert_eq!(parsed["task"]["kind"], "Test");
        assert_eq!(parsed["timeout_seconds"], 60);
        assert!(
            parsed["instructions"]
                .as_str()
                .unwrap()
                .contains("Test Step")
        );
    }
}
