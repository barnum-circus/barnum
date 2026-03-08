//! Task queue runner for GSD.
//!
//! Executes tasks through `agent_pool`, validating transitions and handling timeouts.

mod dispatch;
mod finally;
mod hooks;
mod response;
mod shell;
mod submit;
mod types;

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::num::NonZeroU16;
use std::sync::mpsc;
use std::thread;

use tracing::{error, info, warn};

use crate::docs::generate_step_docs;
use crate::resolved::{Action, Config, Step};
use crate::types::{HookScript, LogTaskId, StepName};
use crate::value_schema::{CompiledSchemas, Task};

use crate::types::StepInputValue;
use dispatch::{TaskContext, dispatch_command_task, dispatch_pool_task};
use finally::run_finally_hook_direct;
use hooks::{call_wake_script, run_post_hook};
use response::{FailureKind, ProcessedSubmit, process_retry, process_submit_result};
use types::{
    InFlight, InFlightResult, PoolConnection, TaskEntry, TaskIdentity, TaskOutcome, TaskState,
};

use types::TaskResult;
pub use types::{PostHookInput, RunnerConfig};

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
            pool_root = %runner_config.agent_pool_root.display(),
            invoker = %runner_config.invoker.description(),
            max_concurrency,
            "starting task queue"
        );

        let (tx, rx) = mpsc::channel();

        let pool = PoolConnection {
            root: runner_config.agent_pool_root.to_path_buf(),
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
            next_task_id: 0,
        };

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

            runner.queue_task(task, None);
        }

        Ok(runner)
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

    // ==================== State Transitions ====================

    /// Add a new task - dispatch immediately if under concurrency, otherwise queue as Pending.
    fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>) {
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
                    finally_script: None,
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
                    finally_script: None,
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
    fn dispatch(&self, task_id: LogTaskId, task: Task) {
        #[expect(clippy::expect_used)] // Invariant: config validation ensures all step names exist
        let step = self.step_map.get(&task.step).expect("[P014] unknown step");

        let tx = self.tx.clone();
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
                let invoker = self.pool.invoker.clone();

                info!(step = %ctx.identity.task.step, "submitting task to pool");

                thread::spawn(move || {
                    dispatch_pool_task(ctx, &docs, timeout, &pool_root, &invoker, &tx);
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

    /// Transition `InFlight` → `WaitingForChildren`.
    #[expect(clippy::expect_used)] // Invariant: task must exist
    fn transition_to_waiting(
        &mut self,
        task_id: LogTaskId,
        pending_children_count: NonZeroU16,
        finally_data: Option<(HookScript, StepInputValue)>,
    ) {
        let entry = self.tasks.get_mut(&task_id).expect("task must exist");
        assert!(matches!(entry.state, TaskState::InFlight(_)));
        entry.state = TaskState::WaitingForChildren {
            pending_children_count,
            finally_data,
        };
        self.in_flight -= 1;
    }

    /// Remove an `InFlight` task (for retry - don't notify parent).
    #[expect(clippy::expect_used)] // Invariant: task must exist
    fn transition_to_done(&mut self, task_id: LogTaskId) -> Option<LogTaskId> {
        let entry = self.tasks.remove(&task_id).expect("task must exist");
        assert!(matches!(entry.state, TaskState::InFlight(_)));
        self.in_flight -= 1;
        entry.parent_id
    }

    // ==================== Key Operations ====================

    /// Handle completion of a task (`InFlight` success with no children, or `WaitingForChildren` with count=0).
    /// Runs finally hook if present; if that spawns tasks, waits for them.
    /// Otherwise removes task and notifies parent.
    #[expect(clippy::panic, clippy::expect_used)] // Finally hook returning invalid tasks is a config bug
    fn handle_completion(
        &mut self,
        task_id: LogTaskId,
        finally_data: Option<(HookScript, StepInputValue)>,
    ) {
        let spawned = if let Some((hook, value)) = finally_data {
            let tasks = run_finally_hook_direct(&hook, &value.0);

            // Validate finally hook spawned tasks
            for task in &tasks {
                assert!(
                    self.step_map.contains_key(&task.step),
                    "[P016] BUG: finally hook returned unknown step '{}' - this is a configuration error",
                    task.step
                );
                if let Err(e) = self.schemas.validate(&task.step, &task.value) {
                    panic!(
                        "[P017] BUG: finally hook returned invalid task for step '{}': {e}",
                        task.step
                    );
                }
            }
            tasks
        } else {
            vec![]
        };

        if spawned.is_empty() {
            // Remove and notify parent
            let entry = self.tasks.remove(&task_id).expect("task must exist");
            if matches!(entry.state, TaskState::InFlight(_)) {
                self.in_flight -= 1;
            }
            if let Some(parent_id) = entry.parent_id {
                self.decrement_parent(parent_id);
            }
        } else {
            // Transition to or update WaitingForChildren state
            #[expect(clippy::unwrap_used, clippy::cast_possible_truncation)]
            let count = NonZeroU16::new(spawned.len() as u16).unwrap();

            let entry = self.tasks.get_mut(&task_id).expect("task must exist");
            if matches!(entry.state, TaskState::InFlight(_)) {
                self.in_flight -= 1;
            }
            entry.state = TaskState::WaitingForChildren {
                pending_children_count: count,
                finally_data: None,
            };

            for child in spawned {
                self.queue_task(child, Some(task_id));
            }
        }
    }

    /// Decrement parent's pending count, run finally or finish when count hits 0.
    #[expect(clippy::expect_used, clippy::panic, clippy::unwrap_used)] // Invariants
    fn decrement_parent(&mut self, parent_id: LogTaskId) {
        let (hit_zero, finally_data) = {
            let entry = self.tasks.get_mut(&parent_id).expect("parent must exist");
            let TaskState::WaitingForChildren {
                pending_children_count,
                finally_data,
            } = &mut entry.state
            else {
                panic!("parent not in WaitingForChildren state");
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
            self.handle_completion(parent_id, finally_data);
        }
    }

    /// Handle task success.
    #[expect(clippy::unwrap_used, clippy::cast_possible_truncation)] // spawned.len() fits in u16
    fn task_succeeded(
        &mut self,
        task_id: LogTaskId,
        step_name: &StepName,
        spawned: Vec<Task>,
        value: StepInputValue,
    ) {
        let finally_hook = self
            .config
            .steps
            .iter()
            .find(|s| &s.name == step_name)
            .and_then(|s| s.finally_hook.clone());
        let finally_data = finally_hook.map(|hook| (hook, value));

        if spawned.is_empty() {
            // No children - handle completion (may run finally)
            self.handle_completion(task_id, finally_data);
        } else {
            // Has children - wait for them
            let count = NonZeroU16::new(spawned.len() as u16).unwrap();
            self.transition_to_waiting(task_id, count, finally_data);
            for child in spawned {
                self.queue_task(child, Some(task_id));
            }
        }
    }

    /// Handle task failure (with optional retry).
    #[expect(clippy::expect_used)] // Invariant: task must exist
    fn task_failed(&mut self, task_id: LogTaskId, retry: Option<Task>) {
        let parent_id = self.tasks.get(&task_id).expect("task must exist").parent_id;

        if let Some(retry_task) = retry {
            self.queue_task(retry_task, parent_id);
            self.transition_to_done(task_id); // Don't notify - retry takes over
        } else {
            // Permanent failure - remove and notify parent
            let entry = self.tasks.remove(&task_id).expect("task must exist");
            if matches!(entry.state, TaskState::InFlight(_)) {
                self.in_flight -= 1;
            }
            if let Some(parent_id) = entry.parent_id {
                self.decrement_parent(parent_id);
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
            match run_post_hook(hook, &post_input) {
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
                self.task_succeeded(task_id, &task.step, spawned, finally_value);
                TaskResult::Completed
            }

            TaskOutcome::Retry(retry_task) => {
                self.task_failed(task_id, Some(retry_task));
                TaskResult::Requeued
            }

            TaskOutcome::Dropped => {
                self.task_failed(task_id, None);
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
