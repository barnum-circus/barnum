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

use std::collections::{HashMap, VecDeque};
use std::io;
use std::sync::mpsc;
use std::thread;

use tracing::{error, info, warn};

use crate::docs::generate_step_docs;
use crate::resolved::{Action, Config, Step};
use crate::types::{LogTaskId, StepName};
use crate::value_schema::{CompiledSchemas, Task};

use dispatch::{TaskContext, dispatch_command_task, dispatch_pool_task};
use finally::{FinallyTracker, run_finally_hook, run_finally_hook_direct};
use hooks::{call_wake_script, run_post_hook};
use response::{FailureKind, ProcessedSubmit, process_retry, process_submit_result};
use types::{InFlightResult, PoolConnection, QueuedTask, TaskIdentity, TaskOutcome};

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
    queue: VecDeque<QueuedTask>,
    pool: PoolConnection,
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
            queue: VecDeque::new(),
            pool,
            max_concurrency,
            in_flight: 0,
            tx,
            rx,
            next_task_id: 0,
            finally_tracker: FinallyTracker::new(),
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

            let id = runner.next_task_id();
            runner.queue.push_back(QueuedTask {
                task,
                id,
                finally_origin_id: None,
            });
        }

        Ok(runner)
    }

    fn pending(&self) -> usize {
        self.queue.len()
    }

    /// Allocate the next task ID.
    #[expect(clippy::missing_const_for_fn)] // &mut self can't be const
    fn next_task_id(&mut self) -> LogTaskId {
        let id = LogTaskId(self.next_task_id);
        self.next_task_id += 1;
        id
    }

    fn submit_pending(&mut self) {
        while self.in_flight < self.max_concurrency {
            let Some(queued) = self.queue.pop_front() else {
                break;
            };
            self.submit_one(queued);
        }
    }

    fn submit_one(&mut self, queued: QueuedTask) {
        let QueuedTask {
            task,
            id: task_id,
            finally_origin_id,
        } = queued;

        #[expect(clippy::expect_used)] // Invariant: config validation ensures all step names exist
        let step = self.step_map.get(&task.step).expect("unknown step");

        let tx = self.tx.clone();
        let identity = TaskIdentity {
            task,
            task_id,
            finally_origin_id,
        };
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
        self.in_flight += 1;
    }

    /// Notify that a descendant of `finally_origin_id` has completed.
    /// Runs finally hook if all descendants are done.
    #[expect(clippy::panic)] // Finally hook returning invalid tasks is a config bug
    fn notify_origin_of_completion(&mut self, finally_origin_id: LogTaskId) {
        if let Some(state) = self
            .finally_tracker
            .record_descendant_done(finally_origin_id)
        {
            let spawned = run_finally_hook(&state);
            for task in spawned {
                // Validate finally hook spawned tasks
                assert!(
                    self.step_map.contains_key(&task.step),
                    "BUG: finally hook returned unknown step '{}' - this is a configuration error",
                    task.step
                );
                if let Err(e) = self.schemas.validate(&task.step, &task.value) {
                    panic!(
                        "BUG: finally hook returned invalid task for step '{}': {e}",
                        task.step
                    );
                }

                let id = self.next_task_id();
                self.queue.push_back(QueuedTask {
                    task,
                    id,
                    finally_origin_id: None, // Finally tasks are new roots
                });
            }
        }
    }

    fn process_result(&mut self, inflight: InFlightResult) -> TaskResult {
        let InFlightResult { identity, result } = inflight;

        let TaskIdentity {
            task,
            task_id,
            finally_origin_id,
        } = identity;

        #[expect(clippy::expect_used)] // Invariant: all queued tasks are validated at entry points
        let step = self.step_map.get(&task.step).expect(
            "BUG: task step must exist - all queued tasks should be validated at entry points",
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
                // Determine finally_origin_id for children
                let child_finally_origin_id = if let Some(finally) = &step.finally_hook {
                    if spawned.is_empty() {
                        // No children - run finally hook immediately
                        let finally_spawned = run_finally_hook_direct(finally, &finally_value.0);
                        for child in finally_spawned {
                            let id = self.next_task_id();
                            self.queue.push_back(QueuedTask {
                                task: child,
                                id,
                                finally_origin_id: None, // Finally tasks are new roots
                            });
                        }
                        finally_origin_id
                    } else {
                        self.finally_tracker.start_tracking(
                            task_id,
                            spawned.len(),
                            finally_value.0,
                            finally.clone(),
                        );
                        Some(task_id)
                    }
                } else {
                    finally_origin_id
                };

                // Queue spawned children
                for child in spawned {
                    let id = self.next_task_id();
                    self.queue.push_back(QueuedTask {
                        task: child,
                        id,
                        finally_origin_id: child_finally_origin_id,
                    });
                }

                // Task succeeded - notify origin
                if let Some(oid) = finally_origin_id {
                    self.notify_origin_of_completion(oid);
                }

                TaskResult::Completed
            }

            TaskOutcome::Retry(retry_task) => {
                // Queue retry with SAME finally_origin_id - origin still waiting
                let id = self.next_task_id();
                self.queue.push_back(QueuedTask {
                    task: retry_task,
                    id,
                    finally_origin_id, // Propagate - origin still waiting
                });

                // Do NOT notify origin - the logical task isn't done, it's retrying
                TaskResult::Requeued
            }

            TaskOutcome::Dropped => {
                // Task failed permanently - notify origin (descendant is done, failed)
                if let Some(oid) = finally_origin_id {
                    self.notify_origin_of_completion(oid);
                }

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
    /// Returns `None` when queue is empty and no tasks are in flight.
    fn next(&mut self) -> Option<Self::Item> {
        self.submit_pending();

        if self.in_flight == 0 {
            return None;
        }

        let result = self.rx.recv().ok()?;
        self.in_flight -= 1;

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
