//! Task queue runner for Barnum.
//!
//! Executes tasks through `troupe`, validating transitions and handling timeouts.
//! The Engine processes worker results, converts them to state log entries,
//! and dispatches new work. A log writer persists every entry for resume.

mod action;
mod hooks;
mod response;

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{self, Write as _};
use std::num::NonZeroU16;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc;
use std::time::Duration;

use barnum_state::{
    FailureReason, FinallyRun, InvalidResponseReason, RetryOrigin, SpawnedOrigin, StateLogConfig,
    StateLogEntry, TaskCompleted, TaskOrigin, TaskSubmitted,
};
use tracing::{error, info};

use crate::config::{
    ActionKind, BashAction, Config, EffectiveOptions, FinallyHook, HookCommand, Step,
};
use crate::types::{LogTaskId, StepInputValue, StepName, Task};

use action::{ActionError, ShellAction, WorkerKind, WorkerResult, spawn_worker};
use hooks::call_wake_script;
use response::{FailureKind, TaskOutcome, TaskSuccess, process_submit_result};

// ==================== Public API ====================

/// Runner configuration (how to run, not what to run).
pub struct RunnerConfig<'a> {
    /// Working directory for command actions (typically the config file's directory).
    pub working_dir: &'a Path,
    /// Optional wake script to call before starting.
    pub wake_script: Option<&'a str>,
    /// Path for state log (NDJSON file for persistence/resume).
    pub state_log_path: &'a Path,
}

// ==================== Internal Types ====================

/// Default maximum concurrent task submissions.
const DEFAULT_MAX_CONCURRENCY: usize = 20;

/// Entry in the task state map.
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    /// How many times this task has been retried (0 for the original attempt).
    retries: u32,
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

/// A pending task dispatch.
struct PendingTask {
    task_id: LogTaskId,
}

/// A pending finally dispatch.
struct PendingFinally {
    parent_id: LogTaskId,
}

/// What to dispatch next.
enum PendingDispatch {
    /// Dispatch a task worker.
    Task(PendingTask),
    /// Dispatch a finally worker for a parent whose children all completed.
    Finally(PendingFinally),
}

// ==================== RunState ====================

/// Pure task-tree state. All methods are state mutations — no I/O.
///
/// Processes `StateLogEntry` values via `apply_entry`, mutating the task tree
/// and accumulating `PendingDispatch` items. Callers drain `pending_dispatches`
/// to do actual work (spawn threads, etc.).
struct RunState {
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    next_task_id: u32,
    pending_dispatches: VecDeque<PendingDispatch>,
}

impl RunState {
    const fn new() -> Self {
        Self {
            tasks: BTreeMap::new(),
            next_task_id: 0,
            pending_dispatches: VecDeque::new(),
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
        let (parent_id, retries) = match &submitted.origin {
            TaskOrigin::Seed => (None, 0),
            TaskOrigin::Spawned(spawned) => {
                if let Some(pid) = &spawned.parent_id {
                    let parent = self
                        .tasks
                        .get(pid)
                        .expect("[P046] spawned child's parent must exist");
                    assert!(
                        matches!(&parent.state, TaskState::WaitingForChildren(..)),
                        "[P049] spawned child's parent not in WaitingForChildren state"
                    );
                }
                (spawned.parent_id, 0)
            }
            TaskOrigin::Retry(retry) => {
                let old = self
                    .tasks
                    .remove(&retry.replaces)
                    .expect("[P042] retry target must exist");
                assert!(
                    matches!(old.state, TaskState::Failed),
                    "[P045] retry target not in Failed state"
                );
                (old.parent_id, old.retries + 1)
            }
        };

        let prev = self.tasks.insert(
            submitted.task_id,
            TaskEntry {
                step: submitted.step.clone(),
                parent_id,
                retries,
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

    /// Apply a single state log entry. Mutates the task tree and queues
    /// pending dispatches. Does not execute dispatches — the caller drains
    /// `pending_dispatches` to spawn actual work.
    fn apply_entry(&mut self, entry: &StateLogEntry, config: &Config) {
        match entry {
            StateLogEntry::Config(_) => {
                // Config is handled by the coordinator, not the engine.
            }
            StateLogEntry::TaskSubmitted(s) => {
                self.advance_id_to(s.task_id.0 + 1);
                self.apply_submitted(s);
                self.pending_dispatches
                    .push_back(PendingDispatch::Task(PendingTask { task_id: s.task_id }));
            }
            StateLogEntry::TaskCompleted(c) => {
                // Remove pending dispatch for this task (replay: completed before dispatched)
                self.pending_dispatches.retain(
                    |d| !matches!(d, PendingDispatch::Task(PendingTask { task_id }) if *task_id == c.task_id),
                );

                // Track max ID from embedded children/retries
                match &c.outcome {
                    barnum_state::TaskOutcome::Success(s) => {
                        for child in &s.children {
                            self.advance_id_to(child.task_id.0 + 1);
                        }
                    }
                    barnum_state::TaskOutcome::Failed(f) => {
                        if let Some(retry) = &f.retry {
                            self.advance_id_to(retry.task_id.0 + 1);
                        }
                    }
                }

                let parent_id = self.apply_completed(c);

                // Queue children/retry for dispatch
                match &c.outcome {
                    barnum_state::TaskOutcome::Success(s) => {
                        for child in &s.children {
                            self.pending_dispatches
                                .push_back(PendingDispatch::Task(PendingTask {
                                    task_id: child.task_id,
                                }));
                        }
                    }
                    barnum_state::TaskOutcome::Failed(f) => {
                        if let Some(retry) = &f.retry {
                            self.pending_dispatches
                                .push_back(PendingDispatch::Task(PendingTask {
                                    task_id: retry.task_id,
                                }));
                        }
                    }
                }

                // For leaf/permanent-failure: walk up the parent chain for finally
                if let Some(pid) = parent_id
                    && let Some(finally_id) = self.walk_up_for_finally(pid, config)
                {
                    self.pending_dispatches
                        .push_back(PendingDispatch::Finally(PendingFinally {
                            parent_id: finally_id,
                        }));
                }
            }
            StateLogEntry::FinallyRun(f) => {
                // Remove pending finally dispatch (replay: completed before dispatched)
                self.pending_dispatches.retain(|d| {
                    !matches!(d, PendingDispatch::Finally(PendingFinally { parent_id }) if *parent_id == f.finally_for)
                });

                for child in &f.children {
                    self.advance_id_to(child.task_id.0 + 1);
                }

                let grandparent_id = self.apply_finally_run(f);

                for child in &f.children {
                    self.pending_dispatches
                        .push_back(PendingDispatch::Task(PendingTask {
                            task_id: child.task_id,
                        }));
                }

                // Walk up from grandparent for further finally detection
                if let Some(gp_id) = grandparent_id
                    && let Some(finally_id) = self.walk_up_for_finally(gp_id, config)
                {
                    self.pending_dispatches
                        .push_back(PendingDispatch::Finally(PendingFinally {
                            parent_id: finally_id,
                        }));
                }
            }
        }
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
    config_json: Arc<serde_json::Value>,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,
    working_dir: PathBuf,
    tx: mpsc::Sender<WorkerResult>,
    max_concurrency: usize,
    in_flight: usize,
    dropped_count: u32,
}

impl<'a> Engine<'a> {
    fn new(
        config: &'a Config,
        config_json: Arc<serde_json::Value>,
        working_dir: PathBuf,
        tx: mpsc::Sender<WorkerResult>,
        max_concurrency: usize,
    ) -> Self {
        Self {
            config,
            config_json,
            step_map: config.step_map(),
            state: RunState::new(),
            working_dir,
            tx,
            max_concurrency,
            in_flight: 0,
            dropped_count: 0,
        }
    }

    /// Apply entries to state and dispatch pending work.
    ///
    /// Used for seed entries and replay. During replay, `in_flight` stays
    /// at 0 (no actual workers), so `flush_dispatches` dispatches any
    /// remaining pending work after all entries are applied.
    fn apply_and_dispatch(&mut self, entries: &[StateLogEntry]) {
        for entry in entries {
            self.state.apply_entry(entry, self.config);
        }
        self.flush_dispatches();
    }

    /// Process a raw worker result. Returns entries to write to the log.
    fn process_worker_result(&mut self, result: WorkerResult) -> Vec<StateLogEntry> {
        self.in_flight = self.in_flight.saturating_sub(1);

        let entries = match result.kind {
            WorkerKind::Task => {
                self.convert_task_result(result.task_id, &result.task, result.result)
            }
            WorkerKind::Finally { parent_id } => {
                self.convert_finally_result(parent_id, result.result.output)
            }
        };

        for entry in &entries {
            self.state.apply_entry(entry, self.config);
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
        action_result: action::ActionResult,
    ) -> Vec<StateLogEntry> {
        let step = self
            .step_map
            .get(&task.step)
            .expect("[P015] task step must exist");
        let effective = EffectiveOptions::resolve(&self.config.options, &step.options);

        let outcome = process_submit_result(action_result, task, step, &effective);

        match outcome {
            TaskOutcome::Success(TaskSuccess {
                spawned,
                finally_value,
            }) => {
                let children: Vec<TaskSubmitted> = spawned
                    .into_iter()
                    .map(|child| {
                        let id = self.state.next_id();
                        TaskSubmitted {
                            task_id: id,
                            step: child.step,
                            value: child.value,
                            origin: TaskOrigin::Spawned(SpawnedOrigin {
                                parent_id: Some(task_id),
                            }),
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
                            origin: TaskOrigin::Retry(RetryOrigin { replaces: task_id }),
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
        output: Result<String, ActionError>,
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
                    origin: TaskOrigin::Spawned(SpawnedOrigin {
                        parent_id: grandparent_id,
                    }),
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
            let Some(dispatch) = self.state.pending_dispatches.pop_front() else {
                break;
            };
            match dispatch {
                PendingDispatch::Task(PendingTask { task_id }) => {
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
                    let retries = entry.retries;
                    let mut task = Task::new(step_name.as_str(), value);
                    task.retries = retries;

                    self.in_flight += 1;
                    self.dispatch_task(task_id, task);
                }
                PendingDispatch::Finally(PendingFinally { parent_id }) => {
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
        let effective = EffectiveOptions::resolve(&self.config.options, &step.options);
        let timeout = effective.timeout.map(Duration::from_secs);
        let tx = self.tx.clone();

        match &step.action {
            ActionKind::Bash(BashAction { script }) => {
                info!(step = %task.step, script = %script, "executing command");
                let action = Box::new(ShellAction {
                    script: script.clone(),
                    step_name: task.step.clone(),
                    config: Arc::clone(&self.config_json),
                    working_dir: self.working_dir.clone(),
                });
                spawn_worker(tx, action, task_id, task, WorkerKind::Task, timeout);
            }
        }
    }

    /// Spawn a finally worker thread.
    #[expect(clippy::expect_used)]
    fn dispatch_finally(&self, parent_id: LogTaskId, task: Task) {
        let step = self.step_map.get(&task.step).expect("[P015] unknown step");
        let FinallyHook::Bash(HookCommand { script }) = step
            .finally_hook
            .as_ref()
            .expect("[P073] finally parent's step must have finally_hook");
        let effective = EffectiveOptions::resolve(&self.config.options, &step.options);
        let timeout = effective.timeout.map(Duration::from_secs);

        info!(step = %task.step, parent = ?parent_id, "dispatching finally worker");
        let action = Box::new(ShellAction {
            script: script.clone(),
            step_name: task.step.clone(),
            config: Arc::clone(&self.config_json),
            working_dir: self.working_dir.clone(),
        });
        spawn_worker(
            self.tx.clone(),
            action,
            parent_id,
            task,
            WorkerKind::Finally { parent_id },
            timeout,
        );
    }

    /// True when all work is done.
    fn is_done(&self) -> bool {
        self.state.pending_dispatches.is_empty() && self.in_flight == 0
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
        FailureKind::InvalidResponse => FailureReason::InvalidResponse(InvalidResponseReason {
            message: "invalid response".to_string(),
        }),
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
    runner_config: &RunnerConfig<'_>,
    initial_tasks: Vec<Task>,
) -> io::Result<()> {
    if let Some(script) = runner_config.wake_script {
        call_wake_script(script)?;
    }

    let max_concurrency = config
        .options
        .max_concurrency
        .unwrap_or(DEFAULT_MAX_CONCURRENCY);

    info!(
        tasks = initial_tasks.len(),
        max_concurrency, "starting task queue"
    );

    let (tx, rx) = mpsc::channel();

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
        config: config_json.clone(),
    });
    write_log(&mut log_writer, &config_entry);

    // Create engine
    let mut engine = Engine::new(
        config,
        Arc::new(config_json),
        runner_config.working_dir.to_path_buf(),
        tx,
        max_concurrency,
    );

    // Validate and submit initial tasks as seed entries
    let mut seed_entries = Vec::with_capacity(initial_tasks.len());
    for task in initial_tasks {
        if !engine.step_map.contains_key(&task.step) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("[E019] unknown step '{}' in initial tasks", task.step),
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
    engine.apply_and_dispatch(&seed_entries);

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
    let config: Config = serde_json::from_value(config_json.clone()).map_err(|e| {
        io::Error::other(format!("[E071] failed to deserialize config from log: {e}"))
    })?;

    if let Some(script) = runner_config.wake_script {
        call_wake_script(script)?;
    }

    let max_concurrency = config
        .options
        .max_concurrency
        .unwrap_or(DEFAULT_MAX_CONCURRENCY);

    info!(
        entries = old_entries.len(),
        max_concurrency, "resuming task queue"
    );

    let (tx, rx) = mpsc::channel();

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
    let mut engine = Engine::new(
        &config,
        Arc::new(config_json),
        runner_config.working_dir.to_path_buf(),
        tx,
        max_concurrency,
    );
    engine.apply_and_dispatch(&old_entries);

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
mod run_state_tests {
    use barnum_state::{
        FinallyRun, RetryOrigin, SpawnedOrigin, StateLogEntry, TaskCompleted, TaskOrigin,
        TaskSubmitted,
    };

    use crate::config::{
        ActionKind, BashAction, Config, FinallyHook, HookCommand, Options, Step, StepOptions,
    };
    use crate::types::{LogTaskId, StepInputValue, StepName};

    use super::{PendingDispatch, PendingFinally, PendingTask, RunState, TaskState};

    // ==================== Helpers ====================

    fn step(name: &str) -> Step {
        Step {
            name: StepName::new(name),
            action: ActionKind::Bash(BashAction {
                script: "true".into(),
            }),
            next: vec![],
            finally_hook: None,
            options: StepOptions::default(),
        }
    }

    fn step_with_finally(name: &str) -> Step {
        Step {
            finally_hook: Some(FinallyHook::Bash(HookCommand {
                script: "echo done".into(),
            })),
            ..step(name)
        }
    }

    fn config(steps: Vec<Step>) -> Config {
        Config {
            options: Options::default(),
            entrypoint: None,
            steps,
        }
    }

    fn val() -> StepInputValue {
        StepInputValue(serde_json::json!({}))
    }

    fn seed(id: u32, step_name: &str) -> StateLogEntry {
        StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: LogTaskId(id),
            step: StepName::new(step_name),
            value: val(),
            origin: TaskOrigin::Seed,
        })
    }

    fn spawned(id: u32, step_name: &str, parent_id: u32) -> TaskSubmitted {
        TaskSubmitted {
            task_id: LogTaskId(id),
            step: StepName::new(step_name),
            value: val(),
            origin: TaskOrigin::Spawned(SpawnedOrigin {
                parent_id: Some(LogTaskId(parent_id)),
            }),
        }
    }

    fn leaf_success(task_id: u32) -> StateLogEntry {
        StateLogEntry::TaskCompleted(TaskCompleted {
            task_id: LogTaskId(task_id),
            outcome: barnum_state::TaskOutcome::Success(barnum_state::TaskSuccess {
                finally_value: val(),
                children: vec![],
            }),
        })
    }

    fn success_with_children(task_id: u32, children: Vec<TaskSubmitted>) -> StateLogEntry {
        StateLogEntry::TaskCompleted(TaskCompleted {
            task_id: LogTaskId(task_id),
            outcome: barnum_state::TaskOutcome::Success(barnum_state::TaskSuccess {
                finally_value: val(),
                children,
            }),
        })
    }

    fn failed_with_retry(task_id: u32, retry: TaskSubmitted) -> StateLogEntry {
        StateLogEntry::TaskCompleted(TaskCompleted {
            task_id: LogTaskId(task_id),
            outcome: barnum_state::TaskOutcome::Failed(barnum_state::TaskFailed {
                reason: barnum_state::FailureReason::Timeout,
                retry: Some(retry),
            }),
        })
    }

    fn failed_permanent(task_id: u32) -> StateLogEntry {
        StateLogEntry::TaskCompleted(TaskCompleted {
            task_id: LogTaskId(task_id),
            outcome: barnum_state::TaskOutcome::Failed(barnum_state::TaskFailed {
                reason: barnum_state::FailureReason::Timeout,
                retry: None,
            }),
        })
    }

    fn retry_task(id: u32, step_name: &str, replaces: u32) -> TaskSubmitted {
        TaskSubmitted {
            task_id: LogTaskId(id),
            step: StepName::new(step_name),
            value: val(),
            origin: TaskOrigin::Retry(RetryOrigin {
                replaces: LogTaskId(replaces),
            }),
        }
    }

    fn finally_run(parent_id: u32, children: Vec<TaskSubmitted>) -> StateLogEntry {
        StateLogEntry::FinallyRun(FinallyRun {
            finally_for: LogTaskId(parent_id),
            children,
        })
    }

    fn has_task_dispatch(state: &RunState, task_id: u32) -> bool {
        state.pending_dispatches.iter().any(
            |d| matches!(d, PendingDispatch::Task(PendingTask { task_id: id }) if *id == LogTaskId(task_id)),
        )
    }

    fn has_finally_dispatch(state: &RunState, parent_id: u32) -> bool {
        state
            .pending_dispatches
            .iter()
            .any(|d| matches!(d, PendingDispatch::Finally(PendingFinally { parent_id: id }) if *id == LogTaskId(parent_id)))
    }

    // ==================== TaskSubmitted ====================

    #[test]
    fn seed_queues_task_dispatch() {
        let cfg = config(vec![step("A")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);

        assert!(state.tasks.contains_key(&LogTaskId(0)));
        assert!(matches!(
            state.tasks[&LogTaskId(0)].state,
            TaskState::Pending(_)
        ));
        assert_eq!(state.pending_dispatches.len(), 1);
        assert!(has_task_dispatch(&state, 0));
        assert_eq!(state.next_task_id, 1);
    }

    #[test]
    fn spawned_child_queues_dispatch() {
        let cfg = config(vec![step("A"), step("B")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "B", 0)]), &cfg);
        state.pending_dispatches.clear();

        // Child 1 is in the map with parent_id 0
        assert!(state.tasks.contains_key(&LogTaskId(1)));
        assert_eq!(state.tasks[&LogTaskId(1)].parent_id, Some(LogTaskId(0)));
    }

    #[test]
    fn retry_replaces_failed_task() {
        let cfg = config(vec![step("A")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.apply_entry(&failed_with_retry(0, retry_task(1, "A", 0)), &cfg);

        // Original removed, retry in map
        assert!(!state.tasks.contains_key(&LogTaskId(0)));
        assert!(state.tasks.contains_key(&LogTaskId(1)));
        assert!(matches!(
            state.tasks[&LogTaskId(1)].state,
            TaskState::Pending(_)
        ));
        assert!(has_task_dispatch(&state, 1));
    }

    #[test]
    fn multiple_seeds_all_queued() {
        let cfg = config(vec![step("A")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.apply_entry(&seed(1, "A"), &cfg);
        state.apply_entry(&seed(2, "A"), &cfg);

        assert_eq!(state.tasks.len(), 3);
        assert_eq!(state.pending_dispatches.len(), 3);
        assert_eq!(state.next_task_id, 3);
    }

    #[test]
    fn id_advancement_handles_gaps() {
        let cfg = config(vec![step("A")]);
        let mut state = RunState::new();

        let entry = StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: LogTaskId(5),
            step: StepName::new("A"),
            value: val(),
            origin: TaskOrigin::Seed,
        });
        state.apply_entry(&entry, &cfg);

        assert_eq!(state.next_task_id, 6);
    }

    // ==================== TaskCompleted — Success ====================

    #[test]
    fn leaf_success_removes_task() {
        let cfg = config(vec![step("A")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.pending_dispatches.clear();

        state.apply_entry(&leaf_success(0), &cfg);

        assert!(state.tasks.is_empty());
        assert!(state.pending_dispatches.is_empty());
    }

    #[test]
    fn success_with_children_transitions_to_waiting() {
        let cfg = config(vec![step("A"), step("B")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.pending_dispatches.clear();

        state.apply_entry(
            &success_with_children(0, vec![spawned(1, "B", 0), spawned(2, "B", 0)]),
            &cfg,
        );

        // Parent in WaitingForChildren with count 2
        assert!(matches!(
            &state.tasks[&LogTaskId(0)].state,
            TaskState::WaitingForChildren(w) if w.pending_children_count.get() == 2
        ));
        // Children in map
        assert!(state.tasks.contains_key(&LogTaskId(1)));
        assert!(state.tasks.contains_key(&LogTaskId(2)));
        // Two child dispatches queued
        assert!(has_task_dispatch(&state, 1));
        assert!(has_task_dispatch(&state, 2));
    }

    #[test]
    fn success_with_children_advances_ids() {
        let cfg = config(vec![step("A"), step("B")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.apply_entry(
            &success_with_children(0, vec![spawned(5, "B", 0), spawned(10, "B", 0)]),
            &cfg,
        );

        assert_eq!(state.next_task_id, 11);
    }

    // ==================== TaskCompleted — Failure ====================

    #[test]
    fn failed_with_retry_inserts_retry() {
        let cfg = config(vec![step("A")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.pending_dispatches.clear();

        state.apply_entry(&failed_with_retry(0, retry_task(1, "A", 0)), &cfg);

        assert!(!state.tasks.contains_key(&LogTaskId(0)));
        assert!(state.tasks.contains_key(&LogTaskId(1)));
        assert_eq!(state.pending_dispatches.len(), 1);
        assert!(has_task_dispatch(&state, 1));
    }

    #[test]
    fn retry_tracks_retry_count() {
        let cfg = config(vec![step("A")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        assert_eq!(state.tasks[&LogTaskId(0)].retries, 0);

        state.apply_entry(&failed_with_retry(0, retry_task(1, "A", 0)), &cfg);
        assert_eq!(state.tasks[&LogTaskId(1)].retries, 1);

        state.apply_entry(&failed_with_retry(1, retry_task(2, "A", 1)), &cfg);
        assert_eq!(state.tasks[&LogTaskId(2)].retries, 2);
    }

    #[test]
    fn failed_permanent_removes_task() {
        let cfg = config(vec![step("A")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.pending_dispatches.clear();

        state.apply_entry(&failed_permanent(0), &cfg);

        assert!(state.tasks.is_empty());
        assert!(state.pending_dispatches.is_empty());
    }

    #[test]
    fn failed_permanent_under_parent_walks_up() {
        let cfg = config(vec![step_with_finally("A"), step("B")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "B", 0)]), &cfg);
        state.pending_dispatches.clear();

        state.apply_entry(&failed_permanent(1), &cfg);

        assert!(has_finally_dispatch(&state, 0));
    }

    // ==================== FinallyRun ====================

    #[test]
    fn finally_no_children_removes_parent() {
        let cfg = config(vec![
            step_with_finally("A"),
            step("B"),
            step_with_finally("Root"),
        ]);
        let mut state = RunState::new();

        // Root(0) → A(1) → B(2). B completes, A's finally runs with no children.
        state.apply_entry(&seed(0, "Root"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "A", 0)]), &cfg);
        state.apply_entry(&success_with_children(1, vec![spawned(2, "B", 1)]), &cfg);
        state.apply_entry(&leaf_success(2), &cfg);
        state.pending_dispatches.clear();

        // A's finally runs with no children
        state.apply_entry(&finally_run(1, vec![]), &cfg);

        // A removed from map
        assert!(!state.tasks.contains_key(&LogTaskId(1)));
    }

    #[test]
    fn finally_with_children_adds_children() {
        let cfg = config(vec![step_with_finally("A"), step("B"), step("C")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "B", 0)]), &cfg);
        state.apply_entry(&leaf_success(1), &cfg);
        state.pending_dispatches.clear();

        // A's finally spawns two children
        let children = vec![
            TaskSubmitted {
                task_id: LogTaskId(2),
                step: StepName::new("C"),
                value: val(),
                origin: TaskOrigin::Spawned(SpawnedOrigin { parent_id: None }),
            },
            TaskSubmitted {
                task_id: LogTaskId(3),
                step: StepName::new("C"),
                value: val(),
                origin: TaskOrigin::Spawned(SpawnedOrigin { parent_id: None }),
            },
        ];
        state.apply_entry(&finally_run(0, children), &cfg);

        // A removed, children added
        assert!(!state.tasks.contains_key(&LogTaskId(0)));
        assert!(state.tasks.contains_key(&LogTaskId(2)));
        assert!(state.tasks.contains_key(&LogTaskId(3)));
        assert!(has_task_dispatch(&state, 2));
        assert!(has_task_dispatch(&state, 3));
    }

    #[test]
    fn finally_no_children_under_grandparent_triggers_grandparent_finally() {
        let cfg = config(vec![
            step_with_finally("GP"),
            step_with_finally("P"),
            step("C"),
        ]);
        let mut state = RunState::new();

        // GP(0) → P(1) → C(2). C completes, P's finally runs with no children.
        state.apply_entry(&seed(0, "GP"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "P", 0)]), &cfg);
        state.apply_entry(&success_with_children(1, vec![spawned(2, "C", 1)]), &cfg);
        state.apply_entry(&leaf_success(2), &cfg);
        state.pending_dispatches.clear();

        // P's finally runs with no children → GP count should reach 0
        state.apply_entry(&finally_run(1, vec![]), &cfg);

        // GP should now need its finally
        assert!(has_finally_dispatch(&state, 0));
    }

    // ==================== Finally Detection ====================

    #[test]
    fn child_complete_triggers_parent_finally() {
        let cfg = config(vec![step_with_finally("A"), step("B")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "B", 0)]), &cfg);
        state.pending_dispatches.clear();

        state.apply_entry(&leaf_success(1), &cfg);

        assert_eq!(state.pending_dispatches.len(), 1);
        assert!(has_finally_dispatch(&state, 0));
    }

    #[test]
    fn child_complete_parent_no_finally_removes_parent() {
        let cfg = config(vec![step("A"), step("B")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "B", 0)]), &cfg);
        state.pending_dispatches.clear();

        state.apply_entry(&leaf_success(1), &cfg);

        // Both removed, no finally
        assert!(state.tasks.is_empty());
        assert!(state.pending_dispatches.is_empty());
    }

    #[test]
    fn child_complete_skips_no_finally_ancestors() {
        let cfg = config(vec![step_with_finally("GP"), step("P"), step("C")]);
        let mut state = RunState::new();

        // GP(0) → P(1) → C(2)
        state.apply_entry(&seed(0, "GP"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "P", 0)]), &cfg);
        state.apply_entry(&success_with_children(1, vec![spawned(2, "C", 1)]), &cfg);
        state.pending_dispatches.clear();

        state.apply_entry(&leaf_success(2), &cfg);

        // P has no finally → removed. GP has finally → dispatch queued.
        assert!(!state.tasks.contains_key(&LogTaskId(1)));
        assert!(!state.tasks.contains_key(&LogTaskId(2)));
        assert!(has_finally_dispatch(&state, 0));
    }

    #[test]
    fn child_complete_parent_still_has_siblings() {
        let cfg = config(vec![step_with_finally("A"), step("B")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.apply_entry(
            &success_with_children(0, vec![spawned(1, "B", 0), spawned(2, "B", 0)]),
            &cfg,
        );
        state.pending_dispatches.clear();

        state.apply_entry(&leaf_success(1), &cfg);

        // Parent still waiting (count 1), no finally yet
        assert!(matches!(
            &state.tasks[&LogTaskId(0)].state,
            TaskState::WaitingForChildren(w) if w.pending_children_count.get() == 1
        ));
        assert!(!has_finally_dispatch(&state, 0));
    }

    #[test]
    fn both_children_complete_then_finally() {
        let cfg = config(vec![step_with_finally("A"), step("B")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.apply_entry(
            &success_with_children(0, vec![spawned(1, "B", 0), spawned(2, "B", 0)]),
            &cfg,
        );
        state.pending_dispatches.clear();

        // First child
        state.apply_entry(&leaf_success(1), &cfg);
        assert!(!has_finally_dispatch(&state, 0));

        // Second child
        state.apply_entry(&leaf_success(2), &cfg);
        assert!(has_finally_dispatch(&state, 0));
    }

    #[test]
    fn no_finally_at_any_level_just_removes() {
        let cfg = config(vec![step("GP"), step("P"), step("C")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "GP"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "P", 0)]), &cfg);
        state.apply_entry(&success_with_children(1, vec![spawned(2, "C", 1)]), &cfg);
        state.pending_dispatches.clear();

        state.apply_entry(&leaf_success(2), &cfg);

        // All removed, no finally dispatches
        assert!(state.tasks.is_empty());
        assert!(state.pending_dispatches.is_empty());
    }

    // ==================== Replay Behavior ====================

    #[test]
    fn replay_completed_removes_stale_task_dispatch() {
        let cfg = config(vec![step("A")]);
        let mut state = RunState::new();

        // Seed queues a dispatch
        state.apply_entry(&seed(0, "A"), &cfg);
        assert!(has_task_dispatch(&state, 0));

        // Completed removes the stale dispatch
        state.apply_entry(&leaf_success(0), &cfg);
        assert!(!has_task_dispatch(&state, 0));
    }

    #[test]
    fn replay_finally_removes_stale_finally_dispatch() {
        let cfg = config(vec![step_with_finally("A"), step("B")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "B", 0)]), &cfg);
        state.apply_entry(&leaf_success(1), &cfg);
        assert!(has_finally_dispatch(&state, 0));

        // FinallyRun removes the stale dispatch
        state.apply_entry(&finally_run(0, vec![]), &cfg);
        assert!(!has_finally_dispatch(&state, 0));
    }

    // ==================== Complex Scenarios ====================

    #[test]
    fn retry_under_parent_preserves_parent_waiting() {
        let cfg = config(vec![step_with_finally("A"), step("B")]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "A"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "B", 0)]), &cfg);
        state.pending_dispatches.clear();

        // Child fails with retry
        state.apply_entry(&failed_with_retry(1, retry_task(2, "B", 1)), &cfg);

        // Parent still waiting (retry inherits parent_id)
        assert!(matches!(
            &state.tasks[&LogTaskId(0)].state,
            TaskState::WaitingForChildren(w) if w.pending_children_count.get() == 1
        ));
        assert!(state.tasks.contains_key(&LogTaskId(2)));
        assert!(!has_finally_dispatch(&state, 0));
    }

    #[test]
    fn deeply_nested_finally_chain() {
        // GGP (finally) → GP (no finally) → P (finally) → C
        let cfg = config(vec![
            step_with_finally("GGP"),
            step("GP"),
            step_with_finally("P"),
            step("C"),
        ]);
        let mut state = RunState::new();

        state.apply_entry(&seed(0, "GGP"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "GP", 0)]), &cfg);
        state.apply_entry(&success_with_children(1, vec![spawned(2, "P", 1)]), &cfg);
        state.apply_entry(&success_with_children(2, vec![spawned(3, "C", 2)]), &cfg);
        state.pending_dispatches.clear();

        // C completes → P has finally, so P's finally fires (not GGP's)
        state.apply_entry(&leaf_success(3), &cfg);

        assert!(has_finally_dispatch(&state, 2)); // P's finally
        assert!(!has_finally_dispatch(&state, 0)); // NOT GGP's
    }

    #[test]
    fn finally_spawns_children_that_complete() {
        let cfg = config(vec![
            step_with_finally("GP"),
            step_with_finally("P"),
            step("C"),
            step("FC"),
        ]);
        let mut state = RunState::new();

        // GP(0) → P(1) → C(2)
        state.apply_entry(&seed(0, "GP"), &cfg);
        state.apply_entry(&success_with_children(0, vec![spawned(1, "P", 0)]), &cfg);
        state.apply_entry(&success_with_children(1, vec![spawned(2, "C", 1)]), &cfg);
        state.apply_entry(&leaf_success(2), &cfg);
        state.pending_dispatches.clear();

        // P's finally spawns FC(3) and FC(4) under GP
        let children = vec![
            TaskSubmitted {
                task_id: LogTaskId(3),
                step: StepName::new("FC"),
                value: val(),
                origin: TaskOrigin::Spawned(SpawnedOrigin {
                    parent_id: Some(LogTaskId(0)),
                }),
            },
            TaskSubmitted {
                task_id: LogTaskId(4),
                step: StepName::new("FC"),
                value: val(),
                origin: TaskOrigin::Spawned(SpawnedOrigin {
                    parent_id: Some(LogTaskId(0)),
                }),
            },
        ];
        state.apply_entry(&finally_run(1, children), &cfg);
        state.pending_dispatches.clear();

        // GP had 1 child (P), now has 2 (FC3, FC4) after adjustment
        assert!(matches!(
            &state.tasks[&LogTaskId(0)].state,
            TaskState::WaitingForChildren(w) if w.pending_children_count.get() == 2
        ));

        // Complete FC(3)
        state.apply_entry(&leaf_success(3), &cfg);
        assert!(matches!(
            &state.tasks[&LogTaskId(0)].state,
            TaskState::WaitingForChildren(w) if w.pending_children_count.get() == 1
        ));
        assert!(!has_finally_dispatch(&state, 0));

        // Complete FC(4) — GP reaches zero, GP's finally should fire
        state.apply_entry(&leaf_success(4), &cfg);
        assert!(has_finally_dispatch(&state, 0));
    }
}
