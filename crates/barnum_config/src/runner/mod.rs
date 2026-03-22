//! Task queue runner for Barnum.
//!
//! Executes tasks through `troupe`, validating transitions and handling timeouts.

mod dispatch;
mod hooks;
mod response;
mod shell;
mod submit;

use std::collections::{BTreeMap, HashMap, VecDeque};
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
use tracing::{error, info};
use troupe_cli::TroupeCli;

use crate::docs::generate_step_docs;
use crate::resolved::{Action, Config, Step};
use crate::types::{LogTaskId, StepInputValue, StepName};
use crate::value_schema::{CompiledSchemas, Task};

use dispatch::{
    WorkerResult, dispatch_command_task, dispatch_finally_task, dispatch_pool_task,
    process_and_finalize,
};
use hooks::call_wake_script;
use response::{FailureKind, TaskOutcome};

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
#[derive(Clone)]
struct PoolConnection {
    root: PathBuf,
    working_dir: PathBuf,
    invoker: Invoker<TroupeCli>,
}

/// Whether a task was permanently dropped (retries exhausted).
#[derive(Debug)]
enum TaskResult {
    /// Task completed or will be retried.
    Handled,
    /// Task was dropped after exhausting retries.
    Dropped,
}

/// Entry in the unified task state map.
struct TaskEntry {
    /// The step this task is executing.
    step: StepName,
    /// Parent task waiting for this task to complete.
    parent_id: Option<LogTaskId>,
    /// `true` = Finally task (no pre-hook, just run the step's `finally_hook` script).
    /// `false` = Step task (run pre-hook, then action).
    ///
    /// The actual `HookScript` is looked up from step config at dispatch time.
    is_finally: bool,
    /// Current state of this task.
    state: TaskState,
}

/// State of a task in the runner.
enum TaskState {
    /// Task waiting to be dispatched, or dispatched and awaiting completion.
    ///
    /// Dispatched tasks have their value consumed (set to Null) — the worker
    /// thread holds the real value. `pending_queue` tracks which Pending tasks
    /// haven't been dispatched yet.
    Pending {
        /// The step input value. Consumed (set to Null) when dispatched.
        value: StepInputValue,
    },
    /// Task completed its action, waiting for children to complete.
    WaitingForChildren {
        /// Number of children still pending.
        pending_children_count: NonZeroU16,
        /// Value to pass to the step's finally hook when all children complete.
        /// `Some` for Step tasks whose step config has a finally hook.
        /// `None` for Finally tasks (no "finally of finally") or steps without a hook.
        /// The `HookScript` is looked up from step config when scheduling the finally.
        finally_value: Option<StepInputValue>,
    },
}

/// Default maximum concurrent task submissions.
///
/// Limits parallel submissions to avoid exhausting inotify instances.
/// Linux defaults to `max_user_instances=128`.
const DEFAULT_MAX_CONCURRENCY: usize = 20;

/// Task-tree state: the set of live tasks and the ID counter.
///
/// Separated from `TaskRunner` to isolate state with no I/O dependencies.
/// All methods are pure state mutations — no I/O, no log writes, no dispatch.
struct RunState {
    /// All task state in one place. Tasks not in this map are fully done.
    /// `BTreeMap` ordering by key = FIFO dispatch order (task IDs are monotonic).
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    /// Counter for assigning unique task IDs.
    next_task_id: u32,
    /// Parents whose children all completed, accumulated during `remove_and_notify_parent`.
    /// Drained by `TaskRunner` to handle finally scheduling (which requires I/O).
    removed_parents: Vec<RemovedParent>,
}

/// A parent task whose last child just completed, ready for finally scheduling.
///
/// Accumulated by `RunState::remove_and_notify_parent` instead of calling
/// `schedule_finally` inline (which would require I/O).
struct RemovedParent {
    task_id: LogTaskId,
    step: StepName,
    parent_id: Option<LogTaskId>,
    finally_value: Option<StepInputValue>,
}

impl RunState {
    const fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            next_task_id: 0,
            removed_parents: Vec::new(),
        }
    }

    /// Allocate the next task ID.
    #[expect(clippy::missing_const_for_fn)] // &mut self can't be const
    fn next_id(&mut self) -> LogTaskId {
        let id = LogTaskId(self.next_task_id);
        self.next_task_id += 1;
        id
    }

    /// Remove a task and notify its parent that a child completed.
    ///
    /// When a parent's `pending_children_count` reaches zero, the parent is
    /// recorded in `removed_parents` (with its `finally_data`) but NOT removed
    /// from the map yet. The caller drives the cascade by draining
    /// `removed_parents`, scheduling finally tasks (which may increment
    /// grandparent counts), and then calling `remove_and_notify_parent` again
    /// on each removed parent.
    ///
    /// This non-recursive design ensures finally scheduling can increment
    /// ancestor counts before those ancestors are removed.
    #[expect(clippy::expect_used, clippy::panic, clippy::unwrap_used)] // Invariants
    fn remove_and_notify_parent(&mut self, task_id: LogTaskId) {
        let entry = self.tasks.remove(&task_id).expect("[P021] task must exist");
        let Some(parent_id) = entry.parent_id else {
            return;
        };

        let parent = self
            .tasks
            .get_mut(&parent_id)
            .expect("[P022] parent task must exist");
        let TaskState::WaitingForChildren {
            pending_children_count,
            finally_value,
        } = &mut parent.state
        else {
            panic!("[P023] parent task not in WaitingForChildren state");
        };

        let new_count = pending_children_count.get() - 1;
        if new_count > 0 {
            *pending_children_count = NonZeroU16::new(new_count).unwrap();
        } else {
            let step = parent.step.clone();
            let fv = finally_value.take();
            self.removed_parents.push(RemovedParent {
                task_id: parent_id,
                step,
                parent_id: parent.parent_id,
                finally_value: fv,
            });
        }
    }
}

/// Internal task queue runner.
///
/// Tasks are submitted concurrently, and results are yielded as they complete.
struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,
    pool: PoolConnection,
    max_concurrency: usize,
    /// Count of dispatched tasks (for concurrency limiting).
    in_flight: usize,
    /// Tasks waiting to be dispatched. Popped by `dispatch_all_pending`.
    pending_queue: VecDeque<LogTaskId>,
    tx: mpsc::Sender<WorkerResult>,
    rx: mpsc::Receiver<WorkerResult>,
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
            state: RunState::new(),
            pool,
            max_concurrency,
            in_flight: 0,
            pending_queue: VecDeque::new(),
            tx,
            rx,
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

            let task_id = LogTaskId(runner.state.next_task_id);
            runner.write_log(&StateLogEntry::TaskSubmitted(TaskSubmitted {
                task_id,
                step: task.step.clone(),
                value: task.value.clone(),
                parent_id: None,
                origin: TaskOrigin::Initial,
            }));
            runner.queue_task(task, None, false);
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
            state: RunState {
                tasks: BTreeMap::new(),
                next_task_id: state.next_task_id,
                removed_parents: Vec::new(),
            },
            pool,
            max_concurrency,
            in_flight: 0,
            pending_queue: VecDeque::new(),
            tx,
            rx,
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
            let has_finally = self
                .step_map
                .get(&waiting.step)
                .and_then(|s| s.finally_hook.as_ref())
                .is_some();
            let finally_value = if has_finally {
                Some(waiting.finally_value)
            } else {
                None
            };

            self.state.tasks.insert(
                waiting.task_id,
                TaskEntry {
                    step: waiting.step,
                    parent_id: waiting.parent_id,
                    is_finally: false,
                    state: TaskState::WaitingForChildren {
                        pending_children_count: waiting.pending_children_count,
                        finally_value,
                    },
                },
            );
        }

        // Load pending tasks (need dispatch)
        for pending in state.pending_tasks {
            let is_finally = matches!(pending.origin, TaskOrigin::Finally { .. });

            self.state.tasks.insert(
                pending.task_id,
                TaskEntry {
                    step: pending.step,
                    parent_id: pending.parent_id,
                    is_finally,
                    state: TaskState::Pending {
                        value: pending.value,
                    },
                },
            );
            self.pending_queue.push_back(pending.task_id);
        }
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
    fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>, is_finally: bool) {
        let id = self.state.next_id();

        if self.in_flight < self.max_concurrency {
            // Dispatch immediately — value goes to the worker, entry stores Null.
            let prev = self.state.tasks.insert(
                id,
                TaskEntry {
                    step: task.step.clone(),
                    parent_id,
                    is_finally,
                    state: TaskState::Pending {
                        value: StepInputValue(serde_json::Value::Null),
                    },
                },
            );
            assert!(prev.is_none(), "task_id collision: {id:?} already in map");
            self.in_flight += 1;
            self.dispatch(id, task);
        } else {
            // Queue for later dispatch — value stays in the entry.
            let prev = self.state.tasks.insert(
                id,
                TaskEntry {
                    step: task.step,
                    parent_id,
                    is_finally,
                    state: TaskState::Pending { value: task.value },
                },
            );
            assert!(prev.is_none(), "task_id collision: {id:?} already in map");
            self.pending_queue.push_back(id);
        }
    }

    /// Dispatch a task to a worker thread.
    ///
    /// Workers receive only the data they need (pre-hook, action-specific params)
    /// and send raw results back. Validation and post-hooks run on the main thread.
    #[expect(clippy::expect_used)] // Invariants
    fn dispatch(&self, task_id: LogTaskId, task: Task) {
        let entry = self
            .state
            .tasks
            .get(&task_id)
            .expect("[P014] task must exist");
        let step = self.step_map.get(&task.step).expect("[P015] unknown step");
        let tx = self.tx.clone();

        if entry.is_finally {
            let script = step
                .finally_hook
                .clone()
                .expect("[P073] finally task's step must have finally_hook");
            let working_dir = self.pool.working_dir.clone();

            info!(step = %task.step, "dispatching finally task");

            thread::spawn(move || {
                dispatch_finally_task(task_id, task, &script, &working_dir, &tx);
            });
            return;
        }

        let pre_hook = step.pre.clone();

        match &step.action {
            Action::Pool { .. } => {
                let docs = generate_step_docs(step, self.config);
                let timeout = step.options.timeout;
                let pool = self.pool.clone();

                info!(step = %task.step, "submitting task to pool");

                thread::spawn(move || {
                    dispatch_pool_task(
                        task_id,
                        task,
                        pre_hook.as_ref(),
                        &docs,
                        timeout,
                        &pool,
                        &tx,
                    );
                });
            }
            Action::Command { script } => {
                let script = script.clone();
                let working_dir = self.pool.working_dir.clone();

                info!(step = %task.step, script = %script, "executing command");

                thread::spawn(move || {
                    dispatch_command_task(
                        task_id,
                        task,
                        pre_hook.as_ref(),
                        &script,
                        &working_dir,
                        &tx,
                    );
                });
            }
        }
    }

    /// Dispatch pending tasks up to max concurrency.
    #[expect(clippy::expect_used, clippy::panic)] // Invariant: queued tasks must exist in Pending state
    fn dispatch_all_pending(&mut self) {
        while self.in_flight < self.max_concurrency {
            let Some(task_id) = self.pending_queue.pop_front() else {
                break;
            };
            let entry = self
                .state
                .tasks
                .get_mut(&task_id)
                .expect("[P070] queued task must exist in map");
            let TaskState::Pending { value } = &mut entry.state else {
                panic!("[P071] queued task not in Pending state");
            };
            let value = std::mem::replace(value, StepInputValue(serde_json::Value::Null));
            let task = Task::new(entry.step.as_str(), value);
            self.in_flight += 1;
            self.dispatch(task_id, task);
        }
    }

    /// Remove a dispatched task (for retry — don't notify parent).
    #[expect(clippy::expect_used)] // Invariant: task must exist
    fn transition_to_done(&mut self, task_id: LogTaskId) -> Option<LogTaskId> {
        let entry = self.state.tasks.remove(&task_id).expect("task must exist");
        assert!(matches!(entry.state, TaskState::Pending { .. }));
        self.in_flight -= 1;
        entry.parent_id
    }

    /// Does this task's step have a finally hook?
    /// Returns false for Finally tasks (no "finally of finally").
    fn has_finally_hook(&self, entry: &TaskEntry) -> bool {
        if entry.is_finally {
            return false; // No "finally of finally"
        }
        self.step_map
            .get(&entry.step)
            .is_some_and(|s| s.finally_hook.is_some())
    }

    /// Schedule a finally task as a sibling of the given task.
    ///
    /// The finally task becomes a child of the original task's parent.
    /// `parent_id` and `step` are passed explicitly because the task may
    /// already be removed from the map (when called from `schedule_removed_finally`).
    fn schedule_finally(
        &mut self,
        task_id: LogTaskId,
        parent_id: Option<LogTaskId>,
        step: &StepName,
        value: StepInputValue,
    ) {
        // Increment parent's pending count (finally becomes another child)
        if let Some(parent_id) = parent_id {
            self.increment_pending_children(parent_id);
        }

        // Create the finally task
        let id = self.state.next_id();

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
            step: step.clone(),
            parent_id,
            is_finally: true,
            state: TaskState::Pending { value },
        };
        self.state.tasks.insert(id, finally_entry);
        self.pending_queue.push_back(id);
    }

    /// Increment a task's `pending_children_count`.
    #[expect(clippy::expect_used, clippy::unwrap_used, clippy::panic)] // Invariants
    fn increment_pending_children(&mut self, task_id: LogTaskId) {
        let entry = self
            .state
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

    // ==================== Key Operations ====================

    /// Process removed parents after a `RunState::remove_and_notify_parent` call.
    ///
    /// Each removed parent may have `finally_data` that needs scheduling.
    /// This handles the I/O side (log writes, task insertion) that `RunState` defers.
    /// Drive the cascade of parent removals after a `remove_and_notify_parent` call.
    ///
    /// For each removed parent: schedule its finally task (if any), then
    /// remove it from the map and notify its own parent. Scheduling finally
    /// first ensures grandparent counts are incremented before the grandparent
    /// is considered for removal.
    fn schedule_removed_finally(&mut self) {
        while let Some(removed) = self.state.removed_parents.pop() {
            if let Some(value) = removed.finally_value {
                self.schedule_finally(removed.task_id, removed.parent_id, &removed.step, value);
            }
            self.state.remove_and_notify_parent(removed.task_id);
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

        let entry = self
            .state
            .tasks
            .get(&task_id)
            .expect("[P024] task must exist");
        let has_finally = self.has_finally_hook(entry);
        let parent_id = entry.parent_id;
        let step = entry.step.clone();

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
            if has_finally {
                self.schedule_finally(task_id, parent_id, &step, value);
            }
            self.state.remove_and_notify_parent(task_id);
            self.schedule_removed_finally();
        } else {
            // Compute child IDs before queuing (IDs are monotonically assigned)
            let first_child_id = self.state.next_task_id;
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

            // Has children - wait for them, storing finally_value
            let count = NonZeroU16::new(spawned.len() as u16).unwrap();
            let finally_value = if has_finally { Some(value) } else { None };

            let entry = self
                .state
                .tasks
                .get_mut(&task_id)
                .expect("[P025] task must exist");
            entry.state = TaskState::WaitingForChildren {
                pending_children_count: count,
                finally_value,
            };
            for child in spawned {
                // Log each spawned child
                let child_id = LogTaskId(self.state.next_task_id);
                self.write_log(&StateLogEntry::TaskSubmitted(TaskSubmitted {
                    task_id: child_id,
                    step: child.step.clone(),
                    value: child.value.clone(),
                    parent_id: Some(task_id),
                    origin: TaskOrigin::Spawned,
                }));
                self.queue_task(child, Some(task_id), false);
            }
        }
    }

    /// Handle task failure (with optional retry).
    #[expect(clippy::expect_used)] // Invariant: task must exist
    fn task_failed(&mut self, task_id: LogTaskId, retry: Option<Task>, failure_kind: FailureKind) {
        let entry = self
            .state
            .tasks
            .get(&task_id)
            .expect("[P026] task must exist");
        let parent_id = entry.parent_id;
        let is_finally = entry.is_finally;

        if let Some(retry_task) = retry {
            // Compute retry task ID before logging
            let retry_task_id = LogTaskId(self.state.next_task_id);

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

            self.queue_task(retry_task, parent_id, is_finally);
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

            // Permanent failure - decrement in_flight, remove and notify parent
            let entry = self
                .state
                .tasks
                .get(&task_id)
                .expect("[P027] task must exist");
            assert!(
                matches!(entry.state, TaskState::Pending { .. }),
                "[P072] completed task not in Pending state"
            );
            self.in_flight -= 1;
            self.state.remove_and_notify_parent(task_id);
            self.schedule_removed_finally();
        }
    }

    /// Process a worker result through validation, post-hooks, and state transitions.
    ///
    /// Workers handle pre-hooks and actions. This method runs the remaining
    /// pipeline (validation, post-hooks, retry logic) and routes the outcome.
    #[expect(clippy::expect_used)] // Invariant: task step must exist
    fn process_result(&mut self, result: WorkerResult) -> TaskResult {
        let WorkerResult {
            task_id,
            task,
            result: submit_result,
        } = result;

        let step = self.step_map.get(&task.step).expect(
            "[P015] BUG: task step must exist - all queued tasks are validated at entry points",
        );

        let outcome = process_and_finalize(
            submit_result,
            &task,
            step,
            self.schemas,
            &self.pool.working_dir,
        );

        match outcome {
            TaskOutcome::Success {
                spawned,
                finally_value,
            } => {
                self.task_succeeded(task_id, spawned, finally_value);
                TaskResult::Handled
            }
            TaskOutcome::Retry(retry_task, failure_kind) => {
                self.task_failed(task_id, Some(retry_task), failure_kind);
                TaskResult::Handled
            }
            TaskOutcome::Dropped(failure_kind) => {
                self.task_failed(task_id, None, failure_kind);
                TaskResult::Dropped
            }
        }
    }
}

impl TaskRunner<'_> {
    /// Run the task queue to completion.
    ///
    /// Dispatches pending tasks, receives results, and processes them until
    /// all tasks are done. Returns an error if any tasks were permanently dropped.
    #[expect(clippy::expect_used)] // Channel closing while tasks in flight is a bug
    fn run(&mut self) -> io::Result<()> {
        let mut completed_count = 0u32;
        let mut dropped_count = 0u32;

        loop {
            self.dispatch_all_pending();
            if self.in_flight == 0 {
                break;
            }
            let result = self
                .rx
                .recv()
                .expect("[P062] channel closed while tasks in flight");
            let task_result = self.process_result(result);
            completed_count += 1;
            if matches!(task_result, TaskResult::Dropped) {
                dropped_count += 1;
            }

            info!(
                "{} {} completed, {} {} remaining",
                completed_count,
                if completed_count == 1 {
                    "task"
                } else {
                    "tasks"
                },
                self.pending_queue.len(),
                if self.pending_queue.len() == 1 {
                    "task"
                } else {
                    "tasks"
                }
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
    runner.run()
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
    runner.run()
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
