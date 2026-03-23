//! Reconstruct runner state from a state log for resume.
//!
//! Replays a log to determine which tasks still need work:
//! - **Pending**: submitted but never completed → needs action dispatch
//! - **Waiting**: completed with children that are still alive → don't re-dispatch
//!
//! Note: This module cannot determine which steps have finally hooks (that
//! requires config). Parents whose children are all done but have no
//! `FinallyRun` entry are treated as done — the Engine's replay handles
//! finally re-dispatch using config.

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::num::NonZeroU16;

use barnum_types::{LogTaskId, StepInputValue, StepName};

use crate::types::{StateLogEntry, TaskCompleted, TaskOrigin, TaskOutcome, TaskSubmitted};

/// Errors that can occur during state reconstruction.
#[derive(Debug, thiserror::Error)]
pub enum ReconstructError {
    /// IO error reading the log.
    #[error("IO error: {0}")]
    Io(#[from] io::Error),
    /// Log file was empty.
    #[error("empty log file")]
    EmptyLog,
    /// First entry was not a Config entry.
    #[error("first entry must be Config")]
    FirstEntryNotConfig,
    /// Config entry appeared more than once.
    #[error("Config appeared more than once")]
    DuplicateConfig,
    /// A task ID appeared more than once in submitted tasks.
    #[error("duplicate task_id {0:?}")]
    DuplicateTaskId(LogTaskId),
    /// A `TaskCompleted` entry referenced an unknown task ID.
    #[error("TaskCompleted for unknown task_id {0:?}")]
    CompletedUnknownTask(LogTaskId),
    /// A `TaskCompleted` entry referenced a task that was already completed.
    #[error("TaskCompleted for already-completed task_id {0:?}")]
    AlreadyCompleted(LogTaskId),
    /// A `FinallyRun` entry referenced an unknown parent task.
    #[error("FinallyRun for unknown task_id {0:?}")]
    FinallyRunUnknownTask(LogTaskId),
}

/// Info extracted from a submitted task (from any source in the log).
struct SubmittedTask {
    step: StepName,
    value: StepInputValue,
    /// Parent task, derived from origin.
    parent_id: Option<LogTaskId>,
    origin: TaskOrigin,
}

/// A task that needs its action dispatched on resume.
#[derive(Debug, Clone)]
pub struct ReconstructedTask {
    /// The task's unique ID (preserved from the original run).
    pub task_id: LogTaskId,
    /// Which step to execute.
    pub step: StepName,
    /// The input value for the task.
    pub value: StepInputValue,
    /// Parent task waiting for this one.
    pub parent_id: Option<LogTaskId>,
    /// How this task was created.
    pub origin: TaskOrigin,
}

/// A task that completed its action but is waiting for children.
///
/// On resume, this task should NOT be re-dispatched. It just needs
/// to be in the task map so parent notifications work correctly.
#[derive(Debug, Clone)]
pub struct WaitingTask {
    /// The task's unique ID.
    pub task_id: LogTaskId,
    /// Which step this task executed.
    pub step: StepName,
    /// Parent task waiting for this one.
    pub parent_id: Option<LogTaskId>,
    /// Number of direct children still alive.
    pub pending_children_count: NonZeroU16,
    /// The (post-pre-hook) input value for scheduling the finally hook.
    ///
    /// On resume, combined with the step's finally hook (from config) to
    /// reconstruct `finally_data` in `WaitingForChildren` state.
    pub finally_value: StepInputValue,
}

/// Full state reconstructed from a log file.
#[derive(Debug)]
pub struct ReconstructedState {
    /// Tasks that need their action dispatched (were pending or in-flight at crash).
    pub pending_tasks: Vec<ReconstructedTask>,
    /// Tasks waiting for children to complete (don't re-dispatch).
    pub waiting_tasks: Vec<WaitingTask>,
    /// Next task ID to use (continues from the log's highest ID + 1).
    pub next_task_id: u32,
}

/// Reconstruct runner state from a state log entry stream.
///
/// Returns the config (as raw JSON) and the reconstructed state.
/// The caller is responsible for deserializing the config to the
/// concrete `Config` type.
///
/// # Algorithm
///
/// 1. Parse all entries, collecting submitted tasks from all sources
///    (top-level `TaskSubmitted`, embedded in `TaskCompleted`, embedded in `FinallyRun`)
/// 2. Identify "alive" tasks (pending or have alive descendants)
/// 3. Classify alive tasks as Pending (need dispatch) or Waiting (have alive children)
///
/// # Errors
///
/// Returns `ReconstructError` for malformed logs (empty, missing config,
/// duplicate IDs, unknown task references).
pub fn reconstruct(
    mut entries: impl Iterator<Item = io::Result<StateLogEntry>>,
) -> Result<(serde_json::Value, ReconstructedState), ReconstructError> {
    // First entry must be Config
    let config = match entries.next() {
        Some(Ok(StateLogEntry::Config(c))) => c.config,
        Some(Ok(_)) => return Err(ReconstructError::FirstEntryNotConfig),
        Some(Err(e)) => return Err(e.into()),
        None => return Err(ReconstructError::EmptyLog),
    };

    let mut submitted: BTreeMap<LogTaskId, SubmittedTask> = BTreeMap::new();
    let mut completed: BTreeMap<LogTaskId, TaskCompleted> = BTreeMap::new();
    let mut finally_ran: BTreeSet<LogTaskId> = BTreeSet::new();
    let mut max_task_id: u32 = 0;

    for entry in entries {
        match entry? {
            StateLogEntry::Config(_) => {
                return Err(ReconstructError::DuplicateConfig);
            }
            StateLogEntry::TaskSubmitted(task) => {
                insert_submitted(&mut submitted, &task, &mut max_task_id)?;
            }
            StateLogEntry::TaskCompleted(c) => {
                if !submitted.contains_key(&c.task_id) {
                    return Err(ReconstructError::CompletedUnknownTask(c.task_id));
                }
                if completed.contains_key(&c.task_id) {
                    return Err(ReconstructError::AlreadyCompleted(c.task_id));
                }
                // Extract embedded children/retries
                match &c.outcome {
                    TaskOutcome::Success(s) => {
                        for child in &s.children {
                            insert_submitted(&mut submitted, child, &mut max_task_id)?;
                        }
                    }
                    TaskOutcome::Failed(f) => {
                        if let Some(retry) = &f.retry {
                            insert_submitted(&mut submitted, retry, &mut max_task_id)?;
                        }
                    }
                }
                completed.insert(c.task_id, c);
            }
            StateLogEntry::FinallyRun(f) => {
                if !submitted.contains_key(&f.finally_for) {
                    return Err(ReconstructError::FinallyRunUnknownTask(f.finally_for));
                }
                for child in &f.children {
                    insert_submitted(&mut submitted, child, &mut max_task_id)?;
                }
                finally_ran.insert(f.finally_for);
            }
        }
    }

    let state = build_state(&submitted, &completed, &finally_ran, max_task_id);
    Ok((config, state))
}

/// Insert a submitted task into the map, deriving `parent_id` from origin.
fn insert_submitted(
    submitted: &mut BTreeMap<LogTaskId, SubmittedTask>,
    task: &TaskSubmitted,
    max_task_id: &mut u32,
) -> Result<(), ReconstructError> {
    if submitted.contains_key(&task.task_id) {
        return Err(ReconstructError::DuplicateTaskId(task.task_id));
    }
    *max_task_id = (*max_task_id).max(task.task_id.0);

    let parent_id = match &task.origin {
        TaskOrigin::Seed => None,
        TaskOrigin::Spawned(spawned) => spawned.parent_id,
        TaskOrigin::Retry(retry) => {
            // Inherit parent from replaced task.
            // The replaced task must already be in the map (it was submitted earlier).
            submitted.get(&retry.replaces).and_then(|s| s.parent_id)
        }
    };

    submitted.insert(
        task.task_id,
        SubmittedTask {
            step: task.step.clone(),
            value: task.value.clone(),
            parent_id,
            origin: task.origin.clone(),
        },
    );
    Ok(())
}

/// Build the reconstructed state from collected data.
fn build_state(
    submitted: &BTreeMap<LogTaskId, SubmittedTask>,
    completed: &BTreeMap<LogTaskId, TaskCompleted>,
    finally_ran: &BTreeSet<LogTaskId>,
    max_task_id: u32,
) -> ReconstructedState {
    // A task is "alive" if it was submitted but not completed,
    // OR if it completed with children that are alive.
    // Parents whose finally has run are NOT kept alive by the finally.
    let alive = compute_alive_set(submitted, completed, finally_ran);

    let mut pending_tasks = Vec::new();
    let mut waiting_tasks = Vec::new();

    for task_id in &alive {
        let Some(sub) = submitted.get(task_id) else {
            continue;
        };

        if let Some(comp) = completed.get(task_id) {
            // Task completed but is alive → it has alive dependents → Waiting
            if let TaskOutcome::Success(ref success) = comp.outcome {
                let alive_count = count_alive_dependents(*task_id, submitted, &alive);
                if let Some(count) = NonZeroU16::new(alive_count) {
                    waiting_tasks.push(WaitingTask {
                        task_id: *task_id,
                        step: sub.step.clone(),
                        parent_id: sub.parent_id,
                        pending_children_count: count,
                        finally_value: success.finally_value.clone(),
                    });
                }
            }
        } else {
            // Task not completed → Pending (needs dispatch)
            pending_tasks.push(ReconstructedTask {
                task_id: *task_id,
                step: sub.step.clone(),
                value: sub.value.clone(),
                parent_id: sub.parent_id,
                origin: sub.origin.clone(),
            });
        }
    }

    let next_task_id = if submitted.is_empty() {
        0
    } else {
        max_task_id + 1
    };

    ReconstructedState {
        pending_tasks,
        waiting_tasks,
        next_task_id,
    }
}

/// Count alive tasks that depend on a given parent task.
fn count_alive_dependents(
    task_id: LogTaskId,
    submitted: &BTreeMap<LogTaskId, SubmittedTask>,
    alive: &BTreeSet<LogTaskId>,
) -> u16 {
    let mut count: u16 = 0;
    for (id, s) in submitted {
        if !alive.contains(id) {
            continue;
        }
        if s.parent_id == Some(task_id) {
            count = count.saturating_add(1);
        }
    }
    count
}

/// Compute the set of "alive" task IDs.
///
/// A task is alive if:
/// - It was submitted but never completed (pending at crash time), OR
/// - It completed successfully and has at least one alive dependent
///   (a task whose `parent_id` points to it)
fn compute_alive_set(
    submitted: &BTreeMap<LogTaskId, SubmittedTask>,
    completed: &BTreeMap<LogTaskId, TaskCompleted>,
    _finally_ran: &BTreeSet<LogTaskId>,
) -> BTreeSet<LogTaskId> {
    let mut alive = BTreeSet::new();

    // Seed: all submitted-but-not-completed tasks are alive
    for id in submitted.keys() {
        if !completed.contains_key(id) {
            alive.insert(*id);
        }
    }

    // Propagate upward: if a task is alive, ancestors waiting for it should be alive too.
    loop {
        let mut changed = false;

        for (task_id, comp) in completed {
            if alive.contains(task_id) {
                continue;
            }

            let TaskOutcome::Success(_) = &comp.outcome else {
                continue;
            };

            // Check if any alive task has this task as parent_id
            let has_alive_dependent = submitted
                .iter()
                .any(|(id, s)| s.parent_id == Some(*task_id) && alive.contains(id));

            if has_alive_dependent {
                alive.insert(*task_id);
                changed = true;
            }
        }

        if !changed {
            break;
        }
    }

    alive
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::types::*;
    use serde_json::json;

    fn config_entry() -> StateLogEntry {
        StateLogEntry::Config(StateLogConfig {
            config: json!({"steps": []}),
        })
    }

    fn submit_seed(task_id: u32, step: &str) -> StateLogEntry {
        StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: LogTaskId(task_id),
            step: StepName::new(step),
            value: StepInputValue(json!({"input": task_id})),
            origin: TaskOrigin::Seed,
        })
    }

    fn complete_success(task_id: u32, children: Vec<TaskSubmitted>) -> StateLogEntry {
        StateLogEntry::TaskCompleted(TaskCompleted {
            task_id: LogTaskId(task_id),
            outcome: TaskOutcome::Success(TaskSuccess {
                finally_value: StepInputValue(json!({"input": task_id})),
                children,
            }),
        })
    }

    fn complete_success_leaf(task_id: u32) -> StateLogEntry {
        complete_success(task_id, vec![])
    }

    fn spawned_task(task_id: u32, step: &str, parent_id: u32) -> TaskSubmitted {
        TaskSubmitted {
            task_id: LogTaskId(task_id),
            step: StepName::new(step),
            value: StepInputValue(json!({"input": task_id})),
            origin: TaskOrigin::Spawned(SpawnedOrigin {
                parent_id: Some(LogTaskId(parent_id)),
            }),
        }
    }

    fn retry_task(task_id: u32, step: &str, replaces: u32) -> TaskSubmitted {
        TaskSubmitted {
            task_id: LogTaskId(task_id),
            step: StepName::new(step),
            value: StepInputValue(json!({"input": task_id})),
            origin: TaskOrigin::Retry(RetryOrigin {
                replaces: LogTaskId(replaces),
            }),
        }
    }

    fn complete_failed(
        task_id: u32,
        reason: FailureReason,
        retry: Option<TaskSubmitted>,
    ) -> StateLogEntry {
        StateLogEntry::TaskCompleted(TaskCompleted {
            task_id: LogTaskId(task_id),
            outcome: TaskOutcome::Failed(TaskFailed { reason, retry }),
        })
    }

    fn finally_run(finally_for: u32, children: Vec<TaskSubmitted>) -> StateLogEntry {
        StateLogEntry::FinallyRun(FinallyRun {
            finally_for: LogTaskId(finally_for),
            children,
        })
    }

    fn run_reconstruct(
        entries: Vec<StateLogEntry>,
    ) -> Result<(serde_json::Value, ReconstructedState), ReconstructError> {
        reconstruct(entries.into_iter().map(Ok))
    }

    // ==================== Basic Scenarios ====================

    #[test]
    fn reconstruct_empty_log_errors() {
        let result = run_reconstruct(vec![]);
        assert!(matches!(result, Err(ReconstructError::EmptyLog)));
    }

    #[test]
    fn reconstruct_config_only_returns_empty_state() {
        let (config, state) = run_reconstruct(vec![config_entry()]).unwrap();
        assert_eq!(config, json!({"steps": []}));
        assert!(state.pending_tasks.is_empty());
        assert!(state.waiting_tasks.is_empty());
        assert_eq!(state.next_task_id, 0);
    }

    #[test]
    fn reconstruct_single_task_pending() {
        let (_, state) = run_reconstruct(vec![config_entry(), submit_seed(0, "Analyze")]).unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(0));
        assert_eq!(state.pending_tasks[0].step, "Analyze");
        assert!(state.pending_tasks[0].parent_id.is_none());
        assert_eq!(state.pending_tasks[0].origin, TaskOrigin::Seed);
        assert!(state.waiting_tasks.is_empty());
        assert_eq!(state.next_task_id, 1);
    }

    #[test]
    fn reconstruct_single_task_completed_returns_empty() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "Analyze"),
            complete_success_leaf(0),
        ])
        .unwrap();

        assert!(state.pending_tasks.is_empty());
        assert!(state.waiting_tasks.is_empty());
    }

    #[test]
    fn reconstruct_multiple_pending_tasks() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "A"),
            submit_seed(1, "B"),
            submit_seed(2, "C"),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 3);
        assert_eq!(state.next_task_id, 3);
    }

    // ==================== Parent-Child Relationships ====================

    #[test]
    fn reconstruct_child_pending_parent_waiting() {
        // Parent completes with child. Child still pending → parent is Waiting.
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "Analyze"),
            complete_success(0, vec![spawned_task(1, "Process", 0)]),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(1));
        assert_eq!(state.pending_tasks[0].parent_id, Some(LogTaskId(0)));

        assert_eq!(state.waiting_tasks.len(), 1);
        assert_eq!(state.waiting_tasks[0].task_id, LogTaskId(0));
        assert_eq!(state.waiting_tasks[0].pending_children_count.get(), 1);
    }

    #[test]
    fn reconstruct_two_children_one_complete_parent_waiting() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "Analyze"),
            complete_success(
                0,
                vec![spawned_task(1, "Process", 0), spawned_task(2, "Process", 0)],
            ),
            complete_success_leaf(1),
        ])
        .unwrap();

        // Only task 2 is pending
        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(2));

        // Parent still waiting (1 alive child)
        assert_eq!(state.waiting_tasks.len(), 1);
        assert_eq!(state.waiting_tasks[0].task_id, LogTaskId(0));
        assert_eq!(state.waiting_tasks[0].pending_children_count.get(), 1);
    }

    #[test]
    fn reconstruct_all_children_complete_parent_done() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "Analyze"),
            complete_success(
                0,
                vec![spawned_task(1, "Process", 0), spawned_task(2, "Process", 0)],
            ),
            complete_success_leaf(1),
            complete_success_leaf(2),
        ])
        .unwrap();

        assert!(state.pending_tasks.is_empty());
        assert!(state.waiting_tasks.is_empty());
    }

    #[test]
    fn reconstruct_grandchild_pending_sets_ancestor_waiting() {
        // Task 0 → spawns 1 → spawns 2 (still pending)
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "A"),
            complete_success(0, vec![spawned_task(1, "B", 0)]),
            complete_success(1, vec![spawned_task(2, "C", 1)]),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(2));

        // Both ancestors are waiting
        assert_eq!(state.waiting_tasks.len(), 2);
        let waiting_ids: BTreeSet<_> = state.waiting_tasks.iter().map(|w| w.task_id).collect();
        assert!(waiting_ids.contains(&LogTaskId(0)));
        assert!(waiting_ids.contains(&LogTaskId(1)));
    }

    // ==================== Retry Chains ====================

    #[test]
    fn reconstruct_failed_with_retry_only_retry_pending() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "A"),
            complete_failed(0, FailureReason::Timeout, Some(retry_task(1, "A", 0))),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(1));
        assert_eq!(
            state.pending_tasks[0].origin,
            TaskOrigin::Retry(RetryOrigin {
                replaces: LogTaskId(0)
            })
        );
        assert!(state.waiting_tasks.is_empty());
    }

    #[test]
    fn reconstruct_failed_without_retry_task_dropped() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "A"),
            complete_failed(0, FailureReason::Timeout, None),
        ])
        .unwrap();

        assert!(state.pending_tasks.is_empty());
        assert!(state.waiting_tasks.is_empty());
    }

    #[test]
    fn reconstruct_retry_of_child_parent_still_waiting() {
        // Task 0 spawns child 1. Child 1 fails, retried as 2.
        // Parent 0 should be waiting for the retry.
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "A"),
            complete_success(0, vec![spawned_task(1, "B", 0)]),
            complete_failed(1, FailureReason::Timeout, Some(retry_task(2, "B", 1))),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(2));

        // Parent 0 is waiting because retry task 2 inherits parent_id=0
        assert_eq!(state.waiting_tasks.len(), 1);
        assert_eq!(state.waiting_tasks[0].task_id, LogTaskId(0));
        assert_eq!(state.waiting_tasks[0].pending_children_count.get(), 1);
    }

    // ==================== FinallyRun ====================

    #[test]
    fn reconstruct_finally_run_with_children() {
        // Task 0 completes with child 1. Child 1 completes.
        // Finally runs for task 0, spawning child 2.
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "A"),
            complete_success(0, vec![spawned_task(1, "B", 0)]),
            complete_success_leaf(1),
            finally_run(0, vec![spawned_task(2, "C", 0)]),
        ])
        .unwrap();

        // Child 2 from finally is pending
        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(2));
    }

    #[test]
    fn reconstruct_finally_run_no_children_all_done() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "A"),
            complete_success_leaf(0),
            finally_run(0, vec![]),
        ])
        .unwrap();

        assert!(state.pending_tasks.is_empty());
        assert!(state.waiting_tasks.is_empty());
    }

    // ==================== Error Cases ====================

    #[test]
    fn reconstruct_duplicate_task_id_errors() {
        let result = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "A"),
            submit_seed(0, "B"), // duplicate
        ]);
        assert!(matches!(result, Err(ReconstructError::DuplicateTaskId(_))));
    }

    #[test]
    fn reconstruct_complete_unknown_task_errors() {
        let result = run_reconstruct(vec![config_entry(), complete_success_leaf(99)]);
        assert!(matches!(
            result,
            Err(ReconstructError::CompletedUnknownTask(_))
        ));
    }

    #[test]
    fn reconstruct_duplicate_config_errors() {
        let result = run_reconstruct(vec![config_entry(), config_entry()]);
        assert!(matches!(result, Err(ReconstructError::DuplicateConfig)));
    }

    #[test]
    fn reconstruct_first_entry_not_config_errors() {
        let result = run_reconstruct(vec![submit_seed(0, "A")]);
        assert!(matches!(result, Err(ReconstructError::FirstEntryNotConfig)));
    }

    #[test]
    fn reconstruct_already_completed_errors() {
        let result = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "A"),
            complete_success_leaf(0),
            complete_success_leaf(0), // duplicate
        ]);
        assert!(matches!(result, Err(ReconstructError::AlreadyCompleted(_))));
    }

    // ==================== Complex Scenarios ====================

    #[test]
    fn reconstruct_deep_nesting_five_levels() {
        // 0 → 1 → 2 → 3 → 4 (pending)
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "A"),
            complete_success(0, vec![spawned_task(1, "A", 0)]),
            complete_success(1, vec![spawned_task(2, "A", 1)]),
            complete_success(2, vec![spawned_task(3, "A", 2)]),
            complete_success(3, vec![spawned_task(4, "A", 3)]),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(4));

        // All 4 ancestors are waiting
        assert_eq!(state.waiting_tasks.len(), 4);
        assert_eq!(state.next_task_id, 5);
    }

    #[test]
    fn reconstruct_next_task_id_correct_with_gaps() {
        // Task IDs 0, 5, 10 from completions with embedded children
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_seed(0, "A"),
            complete_success(0, vec![spawned_task(5, "B", 0), spawned_task(10, "B", 0)]),
        ])
        .unwrap();

        assert_eq!(state.next_task_id, 11);
    }
}
