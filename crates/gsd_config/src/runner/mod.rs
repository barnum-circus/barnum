//! Task queue runner for GSD.
//!
//! Executes tasks through `agent_pool`, validating transitions and handling timeouts.
//!
//! Two APIs are provided:
//! - [`run()`] - Run the queue to completion
//! - [`TaskRunner`] - Iterator over task completions for fine-grained control

mod finally;
mod hooks;
mod response;
mod submit;
mod types;

use std::collections::{HashMap, VecDeque};
use std::io;
use std::path::Path;
use std::sync::mpsc;
use std::thread;

use agent_pool_cli::AgentPoolCli;
use cli_invoker::Invoker;
use tracing::{debug, error, info, warn};

use crate::docs::generate_step_docs;
use crate::resolved::{Action, Config, Step};
use crate::types::LogTaskId;
use crate::value_schema::{CompiledSchemas, Task};

use finally::{FinallyTracker, run_finally_hook};
use hooks::{call_wake_script, run_command_action, run_post_hook, run_pre_hook};
use response::{FailureKind, process_command_response, process_pool_response, process_retry};
use submit::{build_agent_payload, submit_via_cli};
use types::{InFlightResult, QueuedTask, SubmitResult};

// Re-export public types
pub use types::{PostHookInput, RunnerConfig, TaskOutcome, TaskResult};

/// Default maximum concurrent task submissions.
///
/// Limits parallel submissions to avoid exhausting inotify instances.
/// Linux defaults to `max_user_instances=128`.
const DEFAULT_MAX_CONCURRENCY: usize = 20;

/// Task queue runner that yields outcomes as tasks complete.
///
/// Tasks are submitted concurrently, and results are yielded as they complete.
///
/// ```text
/// let mut runner = TaskRunner::new(&config, &schemas, runner_config)?;
/// while let Some(outcome) = runner.next() {
///     println!("Task {} -> {:?}", outcome.task.step, outcome.result);
/// }
/// ```
pub struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a str, &'a Step>,
    queue: VecDeque<QueuedTask>,
    agent_pool_root: &'a Path,
    working_dir: &'a Path,
    invoker: &'a Invoker<AgentPoolCli>,
    max_concurrency: usize,
    in_flight: usize,
    tx: mpsc::Sender<InFlightResult>,
    rx: mpsc::Receiver<InFlightResult>,
    /// Counter for assigning unique task IDs.
    next_task_id: u32,
    /// Tracks pending descendants for tasks with `finally` hooks.
    finally_tracker: FinallyTracker,
}

impl<'a> TaskRunner<'a> {
    /// Create a new task runner.
    ///
    /// # Errors
    ///
    /// Returns an error if the wake script fails.
    pub fn new(
        config: &'a Config,
        schemas: &'a CompiledSchemas,
        runner_config: RunnerConfig<'a>,
    ) -> io::Result<Self> {
        if let Some(script) = runner_config.wake_script {
            call_wake_script(script)?;
        }

        // Pool existence/readiness is checked by submit_via_cli on first task submission
        let max_concurrency = config.max_concurrency.unwrap_or(DEFAULT_MAX_CONCURRENCY);

        info!(
            tasks = runner_config.initial_tasks.len(),
            pool_root = %runner_config.agent_pool_root.display(),
            invoker = %runner_config.invoker.description(),
            max_concurrency,
            "starting task queue"
        );

        let (tx, rx) = mpsc::channel();

        let mut runner = Self {
            config,
            schemas,
            step_map: config.step_map(),
            queue: VecDeque::new(),
            agent_pool_root: runner_config.agent_pool_root,
            working_dir: runner_config.working_dir,
            invoker: runner_config.invoker,
            max_concurrency,
            in_flight: 0,
            tx,
            rx,
            next_task_id: 0,
            finally_tracker: FinallyTracker::new(),
        };

        // Queue initial tasks (no origin since they're root tasks)
        for task in runner_config.initial_tasks {
            let id = runner.next_task_id();
            runner.queue.push_back(QueuedTask {
                task,
                id,
                origin_id: None,
            });
        }

        Ok(runner)
    }

    /// Get the next completed task outcome.
    ///
    /// This submits pending tasks concurrently and returns results as they complete.
    /// Returns `None` when queue is empty and no tasks are in flight.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<TaskOutcome> {
        self.submit_pending();

        if self.in_flight == 0 {
            return None;
        }

        let result = self.rx.recv().ok()?;
        self.in_flight -= 1;

        Some(self.process_result(result))
    }

    /// Returns the number of tasks in the queue (not including in-flight).
    #[must_use]
    pub fn pending(&self) -> usize {
        self.queue.len()
    }

    /// Returns true if queue is empty and no tasks are in flight.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty() && self.in_flight == 0
    }

    /// Allocate the next task ID.
    #[allow(clippy::missing_const_for_fn)] // &mut self can't be const
    fn next_task_id(&mut self) -> LogTaskId {
        let id = LogTaskId(self.next_task_id);
        self.next_task_id += 1;
        id
    }

    #[allow(clippy::too_many_lines)]
    fn submit_pending(&mut self) {
        while self.in_flight < self.max_concurrency {
            let Some(queued) = self.queue.pop_front() else {
                break;
            };

            let QueuedTask {
                task,
                id: task_id,
                origin_id,
            } = queued;

            let Some(step) = self.step_map.get(task.step.as_str()) else {
                error!(step = %task.step, "unknown step, skipping task");
                // Decrement origin tracking if this task was being tracked
                self.decrement_origin(origin_id);
                continue;
            };

            if let Err(e) = self.schemas.validate(&task.step, &task.value) {
                error!(step = %task.step, error = %e, "task validation failed, skipping");
                self.decrement_origin(origin_id);
                continue;
            }

            let step_name = step.name.clone();
            let pre_hook = step.pre.clone();
            let post_hook = step.post.clone();
            let finally_hook = step.finally_hook.clone();
            let options = step.options;

            match &step.action {
                Action::Pool { .. } => {
                    let docs = generate_step_docs(step, self.config);
                    let timeout = options.timeout;
                    let root = self.agent_pool_root.to_path_buf();
                    let invoker = Clone::clone(self.invoker);
                    let tx = self.tx.clone();
                    let original_value = task.value.clone();

                    info!(step = %task.step, "submitting task to pool");

                    thread::spawn(move || {
                        // Run pre hook if present
                        let effective_value = match run_pre_hook(pre_hook.as_ref(), &original_value)
                        {
                            Ok(v) => v,
                            Err(e) => {
                                let _ = tx.send(InFlightResult {
                                    task,
                                    task_id,
                                    origin_id,
                                    step_name,
                                    effective_value: original_value,
                                    result: SubmitResult::PreHookError(e),
                                    post_hook,
                                    finally_hook,
                                });
                                return;
                            }
                        };

                        // Build payload with (possibly modified) value
                        let payload =
                            build_agent_payload(&step_name, &effective_value, &docs, timeout);
                        debug!(payload = %payload, "task payload");

                        let result = submit_via_cli(&root, &payload, &invoker);
                        let _ = tx.send(InFlightResult {
                            task,
                            task_id,
                            origin_id,
                            step_name,
                            effective_value,
                            result: SubmitResult::Pool(result),
                            post_hook,
                            finally_hook,
                        });
                    });
                    self.in_flight += 1;
                }
                Action::Command { script } => {
                    let script = script.clone();
                    let tx = self.tx.clone();
                    let original_value = task.value.clone();
                    let task_step = task.step.clone();
                    let working_dir = self.working_dir.to_path_buf();

                    info!(step = %task.step, script = %script, "executing command");

                    thread::spawn(move || {
                        // Run pre hook if present
                        let effective_value = match run_pre_hook(pre_hook.as_ref(), &original_value)
                        {
                            Ok(v) => v,
                            Err(e) => {
                                let _ = tx.send(InFlightResult {
                                    task,
                                    task_id,
                                    origin_id,
                                    step_name,
                                    effective_value: original_value,
                                    result: SubmitResult::PreHookError(e),
                                    post_hook,
                                    finally_hook,
                                });
                                return;
                            }
                        };

                        let task_json = serde_json::to_string(&serde_json::json!({
                            "kind": task_step,
                            "value": effective_value,
                        }))
                        .unwrap_or_default();

                        let result = run_command_action(&script, &task_json, &working_dir);
                        let _ = tx.send(InFlightResult {
                            task,
                            task_id,
                            origin_id,
                            step_name,
                            effective_value,
                            result: SubmitResult::Command(result),
                            post_hook,
                            finally_hook,
                        });
                    });
                    self.in_flight += 1;
                }
            }
        }
    }

    /// Decrement the pending count for an origin and run finally if done.
    fn decrement_origin(&mut self, origin_id: Option<LogTaskId>) {
        let Some(oid) = origin_id else { return };

        if let Some(state) = self.finally_tracker.decrement(oid) {
            let spawned = run_finally_hook(state);
            for task in spawned {
                let id = self.next_task_id();
                self.queue.push_back(QueuedTask {
                    task,
                    id,
                    origin_id: None, // Finally tasks don't inherit origin
                });
            }
        }
    }

    #[allow(clippy::too_many_lines)]
    fn process_result(&mut self, inflight: InFlightResult) -> TaskOutcome {
        let InFlightResult {
            task,
            task_id,
            origin_id,
            step_name,
            effective_value,
            result,
            post_hook,
            finally_hook,
        } = inflight;

        let Some(step) = self.step_map.get(step_name.as_str()) else {
            return TaskOutcome {
                task,
                result: TaskResult::Skipped {
                    reason: "step no longer exists".to_string(),
                },
            };
        };

        let (task_result, new_tasks, post_input) = match result {
            SubmitResult::Pool(Ok(response)) => {
                process_pool_response(response, &task, &effective_value, step, self.schemas)
            }
            SubmitResult::Pool(Err(e)) => {
                error!(step = %task.step, error = %e, "submit failed");
                let (result, tasks) = process_retry(&task, &step.options, FailureKind::SubmitError);
                let post_input = PostHookInput::Error {
                    input: effective_value.clone(),
                    error: e.to_string(),
                };
                (result, tasks, post_input)
            }
            SubmitResult::Command(Ok(stdout)) => {
                process_command_response(&stdout, &task, &effective_value, step, self.schemas)
            }
            SubmitResult::Command(Err(e)) => {
                error!(step = %task.step, error = %e, "command failed");
                let (result, tasks) = process_retry(&task, &step.options, FailureKind::SubmitError);
                let post_input = PostHookInput::Error {
                    input: effective_value.clone(),
                    error: e.to_string(),
                };
                (result, tasks, post_input)
            }
            SubmitResult::PreHookError(e) => {
                error!(step = %task.step, error = %e, "pre hook failed");
                let (result, tasks) = process_retry(&task, &step.options, FailureKind::SubmitError);
                let post_input = PostHookInput::PreHookError {
                    input: task.value.clone(),
                    error: e,
                };
                (result, tasks, post_input)
            }
        };

        // Run post hook synchronously - it can modify the next tasks
        let (final_result, final_tasks) = if let Some(hook) = post_hook {
            match run_post_hook(&hook, &post_input) {
                Ok(modified) => {
                    // Extract next tasks from post hook output
                    let tasks = extract_next_tasks(&modified);
                    (task_result, tasks)
                }
                Err(e) => {
                    // Post hook failed - trigger retry
                    warn!(step = %task.step, error = %e, "post hook failed");
                    process_retry(&task, &step.options, FailureKind::SubmitError)
                }
            }
        } else {
            (task_result, new_tasks)
        };

        // Determine origin_id for new tasks:
        // - If this task has a finally hook and spawns children, they get this task's ID
        // - Otherwise, children inherit the parent's origin_id
        let child_origin_id = if finally_hook.is_some() && !final_tasks.is_empty() {
            // Set up finally tracking for this task
            self.finally_tracker.start_tracking(
                task_id,
                final_tasks.len(),
                effective_value,
                finally_hook.unwrap_or_default(),
            );
            Some(task_id)
        } else {
            origin_id
        };

        // Queue new tasks with proper origin tracking
        for new_task in final_tasks {
            let id = self.next_task_id();
            self.queue.push_back(QueuedTask {
                task: new_task,
                id,
                origin_id: child_origin_id,
            });
        }

        // This task is done - decrement its origin's pending count
        self.decrement_origin(origin_id);

        TaskOutcome {
            task,
            result: final_result,
        }
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
    runner_config: RunnerConfig<'_>,
) -> io::Result<()> {
    let mut runner = TaskRunner::new(config, schemas, runner_config)?;
    let mut completed_count = 0u32;
    let mut dropped_count = 0u32;

    while let Some(outcome) = runner.next() {
        completed_count += 1;
        if matches!(outcome.result, TaskResult::Dropped { .. }) {
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
    use super::*;
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
