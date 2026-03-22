//! Task queue runner for Barnum.
//!
//! Executes tasks through `troupe`, validating transitions and handling timeouts.
//! The Engine processes worker results, converts them to state log entries,
//! and dispatches new work. A log writer persists every entry for resume.

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
    FailureReason, FinallyRun, StateLogConfig, StateLogEntry, TaskCompleted, TaskOrigin,
    TaskSubmitted,
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

/// Default maximum concurrent task submissions.
const DEFAULT_MAX_CONCURRENCY: usize = 20;

/// Entry in the task state map.
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    state: TaskState,
}

/// State of a task in the runner.
enum TaskState {
    /// Task waiting to be dispatched or in flight.
    Pending(PendingState),
    /// Task completed its action, waiting for children to complete.
    WaitingForChildren(WaitingState),
    /// Task failed and a retry follows. Transient: only exists between
    /// `apply_completed` setting it and retry's `apply_submitted` removing it.
    Failed,
}

struct PendingState {
    value: StepInputValue,
}

struct WaitingState {
    pending_children_count: NonZeroU16,
    finally_value: StepInputValue,
}

/// What to dispatch next.
enum PendingDispatch {
    /// Dispatch a task worker.
    Task { task_id: LogTaskId },
    /// Dispatch a finally worker for a parent whose children all completed.
    Finally { parent_id: LogTaskId },
}

// ==================== RunState ====================

/// Pure task-tree state. All methods are state mutations — no I/O.
struct RunState {
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    next_task_id: u32,
}

impl RunState {
    const fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            next_task_id: 0,
        }
    }

    /// Allocate the next task ID.
    const fn next_id(&mut self) -> LogTaskId {
        let id = LogTaskId(self.next_task_id);
        self.next_task_id += 1;
        id
    }

    /// Ensure `next_task_id` is at least `min`.
    const fn advance_id_to(&mut self, min: u32) {
        if self.next_task_id < min {
            self.next_task_id = min;
        }
    }

    /// Insert a submitted task into the state.
    ///
    /// Derives `parent_id` from the origin:
    /// - `Seed` → None
    /// - `Spawned { parent_id }` → `parent_id` (verified in `WaitingForChildren`)
    /// - `Retry { replaces }` → inherited from replaced task (removed from map)
    #[expect(clippy::expect_used)]
    fn apply_submitted(&mut self, submitted: &TaskSubmitted) {
        let parent_id = match &submitted.origin {
            TaskOrigin::Seed => None,
            TaskOrigin::Spawned { parent_id } => {
                if let Some(pid) = parent_id {
                    let parent = self
                        .tasks
                        .get(pid)
                        .expect("[P046] spawned child's parent must exist");
                    assert!(
                        matches!(&parent.state, TaskState::WaitingForChildren(..)),
                        "[P049] spawned child's parent not in WaitingForChildren state"
                    );
                }
                *parent_id
            }
            TaskOrigin::Retry { replaces } => {
                let old = self
                    .tasks
                    .remove(replaces)
                    .expect("[P042] retry target must exist");
                assert!(
                    matches!(old.state, TaskState::Failed),
                    "[P045] retry target not in Failed state"
                );
                old.parent_id
            }
        };

        let prev = self.tasks.insert(
            submitted.task_id,
            TaskEntry {
                step: submitted.step.clone(),
                parent_id,
                state: TaskState::Pending(PendingState {
                    value: submitted.value.clone(),
                }),
            },
        );
        assert!(
            prev.is_none(),
            "[P035] duplicate task_id {:?}",
            submitted.task_id
        );
    }

    /// Process a task completion. Returns the `parent_id` when the completed task
    /// is removed (leaf success or permanent failure) — used by the Engine to
    /// start the parent-chain walk for finally detection.
    #[expect(clippy::expect_used, clippy::unwrap_used)]
    fn apply_completed(&mut self, completed: &TaskCompleted) -> Option<LogTaskId> {
        let entry = self
            .tasks
            .get_mut(&completed.task_id)
            .expect("[P033] completed task must exist");
        assert!(
            matches!(&entry.state, TaskState::Pending(..)),
            "[P034] completed task not in Pending state"
        );

        match &completed.outcome {
            barnum_state::TaskOutcome::Success(success) if !success.children.is_empty() => {
                // Has children → transition to WaitingForChildren, insert children
                entry.state = TaskState::WaitingForChildren(WaitingState {
                    pending_children_count: NonZeroU16::new(
                        success.children.len().try_into().unwrap(),
                    )
                    .unwrap(),
                    finally_value: success.finally_value.clone(),
                });
                for child in &success.children {
                    self.apply_submitted(child);
                }
                None
            }
            barnum_state::TaskOutcome::Failed(failure) if failure.retry.is_some() => {
                // Failed with retry → mark Failed, insert retry (which removes this task)
                entry.state = TaskState::Failed;
                self.apply_submitted(failure.retry.as_ref().unwrap());
                None
            }
            barnum_state::TaskOutcome::Success(_) | barnum_state::TaskOutcome::Failed(_) => {
                // Leaf success or permanent failure → remove task
                let removed = self
                    .tasks
                    .remove(&completed.task_id)
                    .expect("[P033] task must exist for removal");
                removed.parent_id
            }
        }
    }

    /// Process a `FinallyRun` event. Removes the parent whose finally ran.
    /// If children exist, inserts them (replacing the parent under grandparent).
    /// Returns `grandparent_id` if the grandparent reached zero children.
    #[expect(clippy::expect_used)]
    fn apply_finally_run(&mut self, finally_run: &FinallyRun) -> Option<LogTaskId> {
        let parent = self
            .tasks
            .remove(&finally_run.finally_for)
            .expect("[P058] FinallyRun target must exist");
        let grandparent_id = parent.parent_id;

        if finally_run.children.is_empty() {
            // No children from the finally. Notify grandparent.
            if let Some(gp_id) = grandparent_id {
                return self.decrement_child_count(gp_id);
            }
        } else {
            // Children replace the parent under the grandparent.
            for child in &finally_run.children {
                self.apply_submitted(child);
            }
            if let Some(gp_id) = grandparent_id {
                // Count adjustment: -1 (parent removed) + N (new children) = delta N-1
                #[expect(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                let delta = finally_run.children.len() as i16 - 1;
                self.adjust_child_count(gp_id, delta);
            }
        }
        None
    }

    /// Walk up the parent chain from a completed child's parent.
    /// Decrements the parent's child count. If the parent reaches zero:
    ///   - Has a finally script → return its ID (stop walking)
    ///   - No finally script → remove it, continue to grandparent
    ///
    /// Returns `None` if no ancestor needs a finally.
    #[expect(clippy::expect_used)]
    fn walk_up_for_finally(
        &mut self,
        mut parent_id: LogTaskId,
        config: &Config,
    ) -> Option<LogTaskId> {
        let step_map = config.step_map();
        loop {
            self.decrement_child_count(parent_id)?;

            let entry = self
                .tasks
                .get(&parent_id)
                .expect("[P059] parent must exist");
            let has_finally = step_map
                .get(&entry.step)
                .is_some_and(|s| s.finally_hook.is_some());
            if has_finally {
                return Some(parent_id);
            }

            // No finally — remove this ancestor and continue up.
            let removed = self
                .tasks
                .remove(&parent_id)
                .expect("[P059] parent must exist for removal");
            match removed.parent_id {
                Some(gp_id) => parent_id = gp_id,
                None => return None, // reached root
            }
        }
    }

    /// Decrements a task's `pending_children_count`. Returns `Some(task_id)`
    /// if the count reached zero, `None` otherwise.
    #[expect(clippy::expect_used, clippy::panic)]
    fn decrement_child_count(&mut self, task_id: LogTaskId) -> Option<LogTaskId> {
        let entry = self
            .tasks
            .get_mut(&task_id)
            .expect("[P060] task must exist");
        match &mut entry.state {
            TaskState::WaitingForChildren(w) => {
                let count = w.pending_children_count.get() - 1;
                if let Some(new_count) = NonZeroU16::new(count) {
                    w.pending_children_count = new_count;
                    None
                } else {
                    Some(task_id)
                }
            }
            _ => panic!("[P061] decrement on non-WaitingForChildren task"),
        }
    }

    /// Adjusts a task's `pending_children_count` by a delta.
    #[expect(clippy::expect_used, clippy::panic)]
    fn adjust_child_count(&mut self, task_id: LogTaskId, delta: i16) {
        if delta == 0 {
            return;
        }
        let entry = self
            .tasks
            .get_mut(&task_id)
            .expect("[P066] task must exist");
        match &mut entry.state {
            TaskState::WaitingForChildren(w) => {
                #[expect(clippy::cast_sign_loss)]
                let new_count = (w.pending_children_count.get().cast_signed() + delta) as u16;
                w.pending_children_count =
                    NonZeroU16::new(new_count).expect("[P067] child count underflowed");
            }
            _ => panic!("[P068] adjust on non-WaitingForChildren task"),
        }
    }
}

// ==================== Engine ====================

/// Execution engine: processes entries, manages state, dispatches workers.
///
/// The Engine converts raw worker results into state log entries, applies
/// them to `RunState`, and dispatches new work. It does NOT write log entries —
/// the coordinator writes entries returned by `process_worker_result`.
struct Engine<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,
    pool: PoolConnection,
    tx: mpsc::Sender<WorkerResult>,
    max_concurrency: usize,
    in_flight: usize,
    pending_dispatches: VecDeque<PendingDispatch>,
    dropped_count: u32,
}

impl<'a> Engine<'a> {
    fn new(
        config: &'a Config,
        schemas: &'a CompiledSchemas,
        pool: PoolConnection,
        tx: mpsc::Sender<WorkerResult>,
        max_concurrency: usize,
    ) -> Self {
        Self {
            config,
            schemas,
            step_map: config.step_map(),
            state: RunState::new(),
            pool,
            tx,
            max_concurrency,
            in_flight: 0,
            pending_dispatches: VecDeque::new(),
            dropped_count: 0,
        }
    }

    /// Apply a batch of entries (used for seed/replay).
    ///
    /// Processes all entries, then flushes dispatches. During replay,
    /// `in_flight` stays at 0 (no actual workers), so `flush_dispatches`
    /// dispatches any remaining pending work.
    fn apply_entries(&mut self, entries: &[StateLogEntry]) {
        for entry in entries {
            self.apply_entry(entry);
        }
        self.flush_dispatches();
    }

    /// Apply a single entry to state, queueing dispatches as needed.
    fn apply_entry(&mut self, entry: &StateLogEntry) {
        match entry {
            StateLogEntry::Config(_) => {
                // Config is handled by the coordinator, not the engine.
            }
            StateLogEntry::TaskSubmitted(s) => {
                self.state.advance_id_to(s.task_id.0 + 1);
                self.state.apply_submitted(s);
                self.pending_dispatches
                    .push_back(PendingDispatch::Task { task_id: s.task_id });
            }
            StateLogEntry::TaskCompleted(c) => {
                self.in_flight = self.in_flight.saturating_sub(1);
                // Remove pending dispatch for this task (it completed before being dispatched,
                // which happens during replay)
                self.pending_dispatches.retain(
                    |d| !matches!(d, PendingDispatch::Task { task_id } if *task_id == c.task_id),
                );

                // Track max ID from embedded children/retries
                match &c.outcome {
                    barnum_state::TaskOutcome::Success(s) => {
                        for child in &s.children {
                            self.state.advance_id_to(child.task_id.0 + 1);
                        }
                    }
                    barnum_state::TaskOutcome::Failed(f) => {
                        if let Some(retry) = &f.retry {
                            self.state.advance_id_to(retry.task_id.0 + 1);
                        }
                    }
                }

                let parent_id = self.state.apply_completed(c);

                // Queue children/retry for dispatch
                match &c.outcome {
                    barnum_state::TaskOutcome::Success(s) => {
                        for child in &s.children {
                            self.pending_dispatches.push_back(PendingDispatch::Task {
                                task_id: child.task_id,
                            });
                        }
                    }
                    barnum_state::TaskOutcome::Failed(f) => {
                        if let Some(retry) = &f.retry {
                            self.pending_dispatches.push_back(PendingDispatch::Task {
                                task_id: retry.task_id,
                            });
                        }
                    }
                }

                // For leaf/permanent-failure: walk up the parent chain for finally
                if let Some(pid) = parent_id
                    && let Some(finally_id) = self.state.walk_up_for_finally(pid, self.config)
                {
                    self.pending_dispatches.push_back(PendingDispatch::Finally {
                        parent_id: finally_id,
                    });
                }
            }
            StateLogEntry::FinallyRun(f) => {
                self.in_flight = self.in_flight.saturating_sub(1);
                // Remove pending finally dispatch (it completed during replay)
                self.pending_dispatches.retain(|d| {
                    !matches!(d, PendingDispatch::Finally { parent_id } if *parent_id == f.finally_for)
                });

                for child in &f.children {
                    self.state.advance_id_to(child.task_id.0 + 1);
                }

                let grandparent_id = self.state.apply_finally_run(f);

                for child in &f.children {
                    self.pending_dispatches.push_back(PendingDispatch::Task {
                        task_id: child.task_id,
                    });
                }

                // Walk up from grandparent for further finally detection
                if let Some(gp_id) = grandparent_id
                    && let Some(finally_id) = self.state.walk_up_for_finally(gp_id, self.config)
                {
                    self.pending_dispatches.push_back(PendingDispatch::Finally {
                        parent_id: finally_id,
                    });
                }
            }
        }
    }

    /// Process a raw worker result. Returns entries to write to the log.
    fn process_worker_result(&mut self, result: WorkerResult) -> Vec<StateLogEntry> {
        let WorkerResult {
            task_id,
            task,
            result: submit_result,
        } = result;

        let entries = match submit_result {
            dispatch::SubmitResult::Finally { value, output } => {
                self.convert_finally_result(task_id, value, output)
            }
            other => self.convert_task_result(task_id, &task, other),
        };

        for entry in &entries {
            self.apply_entry(entry);
        }
        self.flush_dispatches();
        entries
    }

    /// Convert a regular task result into log entries.
    #[expect(clippy::expect_used)]
    fn convert_task_result(
        &mut self,
        task_id: LogTaskId,
        task: &Task,
        submit_result: dispatch::SubmitResult,
    ) -> Vec<StateLogEntry> {
        let step = self
            .step_map
            .get(&task.step)
            .expect("[P015] task step must exist");

        let outcome = process_and_finalize(
            submit_result,
            task,
            step,
            self.schemas,
            &self.pool.working_dir,
        );

        match outcome {
            TaskOutcome::Success {
                spawned,
                finally_value,
            } => {
                let children: Vec<TaskSubmitted> = spawned
                    .into_iter()
                    .map(|child| {
                        let id = self.state.next_id();
                        TaskSubmitted {
                            task_id: id,
                            step: child.step,
                            value: child.value,
                            origin: TaskOrigin::Spawned {
                                parent_id: Some(task_id),
                            },
                        }
                    })
                    .collect();

                vec![StateLogEntry::TaskCompleted(TaskCompleted {
                    task_id,
                    outcome: barnum_state::TaskOutcome::Success(barnum_state::TaskSuccess {
                        finally_value,
                        children,
                    }),
                })]
            }
            TaskOutcome::Retry(retry_task, failure_kind) => {
                let retry_id = self.state.next_id();
                vec![StateLogEntry::TaskCompleted(TaskCompleted {
                    task_id,
                    outcome: barnum_state::TaskOutcome::Failed(barnum_state::TaskFailed {
                        reason: map_failure(failure_kind),
                        retry: Some(TaskSubmitted {
                            task_id: retry_id,
                            step: retry_task.step,
                            value: retry_task.value,
                            origin: TaskOrigin::Retry { replaces: task_id },
                        }),
                    }),
                })]
            }
            TaskOutcome::Dropped(failure_kind) => {
                self.dropped_count += 1;
                vec![StateLogEntry::TaskCompleted(TaskCompleted {
                    task_id,
                    outcome: barnum_state::TaskOutcome::Failed(barnum_state::TaskFailed {
                        reason: map_failure(failure_kind),
                        retry: None,
                    }),
                })]
            }
        }
    }

    /// Convert a finally worker result into log entries.
    #[expect(clippy::expect_used)]
    fn convert_finally_result(
        &mut self,
        parent_id: LogTaskId,
        _value: StepInputValue,
        output: Result<String, String>,
    ) -> Vec<StateLogEntry> {
        let raw_children = match output {
            Ok(stdout) => match json5::from_str::<Vec<Task>>(&stdout) {
                Ok(tasks) => {
                    info!(parent = ?parent_id, count = tasks.len(), "finally hook completed");
                    tasks
                }
                Err(e) => {
                    tracing::warn!(parent = ?parent_id, error = %e,
                        "finally hook output not parseable, treating as empty");
                    vec![]
                }
            },
            Err(e) => {
                error!(parent = ?parent_id, error = %e, "finally hook failed");
                vec![]
            }
        };

        // Look up grandparent before the FinallyRun removes the parent
        let grandparent_id = self
            .state
            .tasks
            .get(&parent_id)
            .expect("[P058] finally target must exist")
            .parent_id;

        let children: Vec<TaskSubmitted> = raw_children
            .into_iter()
            .map(|child| {
                let id = self.state.next_id();
                TaskSubmitted {
                    task_id: id,
                    step: child.step,
                    value: child.value,
                    origin: TaskOrigin::Spawned {
                        parent_id: grandparent_id,
                    },
                }
            })
            .collect();

        vec![StateLogEntry::FinallyRun(FinallyRun {
            finally_for: parent_id,
            children,
        })]
    }

    /// Dispatch pending work up to `max_concurrency`.
    #[expect(clippy::expect_used, clippy::panic)]
    fn flush_dispatches(&mut self) {
        while self.in_flight < self.max_concurrency {
            let Some(dispatch) = self.pending_dispatches.pop_front() else {
                break;
            };
            match dispatch {
                PendingDispatch::Task { task_id } => {
                    let entry = self
                        .state
                        .tasks
                        .get_mut(&task_id)
                        .expect("[P064] pending task not in map");
                    let TaskState::Pending(pending) = &mut entry.state else {
                        panic!("[P065] pending task not in Pending state");
                    };
                    let value = std::mem::replace(
                        &mut pending.value,
                        StepInputValue(serde_json::Value::Null),
                    );
                    let step_name = entry.step.clone();
                    let task = Task::new(step_name.as_str(), value);

                    self.in_flight += 1;
                    self.dispatch_task(task_id, task);
                }
                PendingDispatch::Finally { parent_id } => {
                    let entry = self
                        .state
                        .tasks
                        .get(&parent_id)
                        .expect("[P063] pending finally parent not in map");
                    let TaskState::WaitingForChildren(waiting) = &entry.state else {
                        panic!("[P069] pending finally not in WaitingForChildren state");
                    };
                    let finally_value = waiting.finally_value.clone();
                    let step_name = entry.step.clone();
                    let task = Task::new(step_name.as_str(), finally_value);

                    self.in_flight += 1;
                    self.dispatch_finally(parent_id, task);
                }
            }
        }
    }

    /// Spawn a task worker thread.
    #[expect(clippy::expect_used)]
    fn dispatch_task(&self, task_id: LogTaskId, task: Task) {
        let step = self.step_map.get(&task.step).expect("[P015] unknown step");
        let tx = self.tx.clone();

        match &step.action {
            Action::Pool { .. } => {
                let pre_hook = step.pre.clone();
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
                let pre_hook = step.pre.clone();
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

    /// Spawn a finally worker thread.
    #[expect(clippy::expect_used)]
    fn dispatch_finally(&self, parent_id: LogTaskId, task: Task) {
        let step = self.step_map.get(&task.step).expect("[P015] unknown step");
        let script = step
            .finally_hook
            .clone()
            .expect("[P073] finally parent's step must have finally_hook");
        let working_dir = self.pool.working_dir.clone();
        let tx = self.tx.clone();

        info!(step = %task.step, parent = ?parent_id, "dispatching finally worker");
        thread::spawn(move || {
            dispatch_finally_task(parent_id, task, &script, &working_dir, &tx);
        });
    }

    /// True when all work is done.
    fn is_done(&self) -> bool {
        self.pending_dispatches.is_empty() && self.in_flight == 0
    }

    /// Compute the workflow result.
    fn compute_result(&self) -> io::Result<()> {
        if self.dropped_count > 0 {
            error!(
                dropped_count = self.dropped_count,
                "task queue completed with dropped tasks"
            );
            Err(io::Error::other(format!(
                "[E018] {} task(s) were dropped (retries exhausted)",
                self.dropped_count
            )))
        } else {
            Ok(())
        }
    }
}

// ==================== Helpers ====================

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

/// Write a state log entry, logging errors.
fn write_log(writer: &mut io::BufWriter<std::fs::File>, entry: &StateLogEntry) {
    if let Err(e) = barnum_state::write_entry(writer, entry) {
        error!(error = %e, "failed to write state log entry");
    }
}

// ==================== Public API ====================

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
    if let Some(script) = runner_config.wake_script {
        call_wake_script(script)?;
    }

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

    // Open state log
    let mut log_writer = {
        let file = std::fs::File::create(runner_config.state_log_path)?;
        io::BufWriter::new(file)
    };
    info!(state_log = %runner_config.state_log_path.display(), "state log");

    // Write config entry
    let config_json =
        serde_json::to_value(config).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let config_entry = StateLogEntry::Config(StateLogConfig {
        config: config_json,
    });
    write_log(&mut log_writer, &config_entry);

    // Create engine
    let mut engine = Engine::new(config, schemas, pool, tx, max_concurrency);

    // Validate and submit initial tasks as seed entries
    let mut seed_entries = Vec::with_capacity(initial_tasks.len());
    for task in initial_tasks {
        if !engine.step_map.contains_key(&task.step) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("[E019] unknown step '{}' in initial tasks", task.step),
            ));
        }
        if let Err(e) = schemas.validate(&task.step, &task.value) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("[E020] initial task validation failed: {e}"),
            ));
        }

        let id = engine.state.next_id();
        let entry = StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: id,
            step: task.step,
            value: task.value,
            origin: TaskOrigin::Seed,
        });
        write_log(&mut log_writer, &entry);
        seed_entries.push(entry);
    }

    // Apply seed entries (queues dispatches, then flushes)
    engine.apply_entries(&seed_entries);

    // Main loop
    run_loop(&mut engine, &rx, &mut log_writer)
}

/// Resume a run from a state log file.
///
/// Reads the old log, replays it through the engine, and continues
/// executing pending tasks.
///
/// # Errors
///
/// Returns an error if the log is malformed, config deserialization fails,
/// or any I/O error occurs.
pub fn resume(old_log_path: &Path, runner_config: &RunnerConfig<'_>) -> io::Result<()> {
    // 1. Read old log entries
    let file = std::fs::File::open(old_log_path)?;
    let old_entries: Vec<StateLogEntry> =
        barnum_state::read_entries(file).collect::<Result<Vec<_>, _>>()?;

    if old_entries.is_empty() {
        return Err(io::Error::other("[E070] empty state log"));
    }

    // 2. Extract config from first entry
    let config_json = match &old_entries[0] {
        StateLogEntry::Config(c) => c.config.clone(),
        _ => return Err(io::Error::other("[E070] first entry must be Config")),
    };
    let config: Config = serde_json::from_value(config_json).map_err(|e| {
        io::Error::other(format!("[E071] failed to deserialize config from log: {e}"))
    })?;
    let schemas = CompiledSchemas::compile(&config)?;

    if let Some(script) = runner_config.wake_script {
        call_wake_script(script)?;
    }

    let max_concurrency = config.max_concurrency.unwrap_or(DEFAULT_MAX_CONCURRENCY);

    info!(
        entries = old_entries.len(),
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

    // 3. Open new state log and copy old entries
    info!(state_log = %runner_config.state_log_path.display(), "state log");
    let mut log_writer = {
        let mut writer = io::BufWriter::new(std::fs::File::create(runner_config.state_log_path)?);
        let old_content = std::fs::read(old_log_path)?;
        writer.write_all(&old_content)?;
        writer.flush()?;
        writer
    };

    // 4. Create engine and replay old entries
    let mut engine = Engine::new(&config, &schemas, pool, tx, max_concurrency);
    engine.apply_entries(&old_entries);

    // 5. Continue with main loop
    run_loop(&mut engine, &rx, &mut log_writer)
}

/// Shared main loop: dispatch pending, receive results, write entries.
#[expect(clippy::expect_used)]
fn run_loop(
    engine: &mut Engine<'_>,
    rx: &mpsc::Receiver<WorkerResult>,
    log_writer: &mut io::BufWriter<std::fs::File>,
) -> io::Result<()> {
    let mut completed_count = 0u32;

    loop {
        if engine.is_done() {
            break;
        }
        let result = rx
            .recv()
            .expect("[P062] channel closed while tasks in flight");
        let entries = engine.process_worker_result(result);
        for entry in &entries {
            write_log(log_writer, entry);
        }
        completed_count += 1;

        info!(
            "{} {} completed, {} in flight",
            completed_count,
            if completed_count == 1 {
                "task"
            } else {
                "tasks"
            },
            engine.in_flight,
        );
    }

    let result = engine.compute_result();
    if result.is_ok() {
        info!(total = completed_count, "task queue complete");
    }
    result
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
