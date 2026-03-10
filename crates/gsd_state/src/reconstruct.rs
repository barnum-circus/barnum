//! Reconstruct runner state from a state log for resume.
//!
//! Replays a log to determine which tasks still need work:
//! - **Pending**: submitted but never completed → needs action dispatch
//! - **Waiting**: completed with children that are still alive → don't re-dispatch

use std::collections::{BTreeMap, BTreeSet};
use std::io;
use std::num::NonZeroU16;

use gsd_types::{LogTaskId, StepInputValue, StepName};

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
    /// A `TaskSubmitted` entry reused an existing task ID.
    #[error("duplicate task_id {0:?}")]
    DuplicateTaskId(LogTaskId),
    /// A `TaskCompleted` entry referenced an unknown task ID.
    #[error("TaskCompleted for unknown task_id {0:?}")]
    CompletedUnknownTask(LogTaskId),
    /// A `TaskCompleted` entry referenced a task that was already completed.
    #[error("TaskCompleted for already-completed task_id {0:?}")]
    AlreadyCompleted(LogTaskId),
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
/// 1. Parse all entries, building submitted/completed maps
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

    // Collect all submitted and completed entries
    let mut submitted: BTreeMap<LogTaskId, TaskSubmitted> = BTreeMap::new();
    let mut completed: BTreeMap<LogTaskId, TaskCompleted> = BTreeMap::new();
    let mut max_task_id: u32 = 0;

    for entry in entries {
        match entry? {
            StateLogEntry::Config(_) => {
                return Err(ReconstructError::DuplicateConfig);
            }
            StateLogEntry::TaskSubmitted(task) => {
                if submitted.contains_key(&task.task_id) {
                    return Err(ReconstructError::DuplicateTaskId(task.task_id));
                }
                max_task_id = max_task_id.max(task.task_id.0);
                submitted.insert(task.task_id, task);
            }
            StateLogEntry::TaskCompleted(c) => {
                if !submitted.contains_key(&c.task_id) {
                    return Err(ReconstructError::CompletedUnknownTask(c.task_id));
                }
                if completed.contains_key(&c.task_id) {
                    return Err(ReconstructError::AlreadyCompleted(c.task_id));
                }
                completed.insert(c.task_id, c);
            }
        }
    }

    let state = build_state(&submitted, &completed, max_task_id);
    Ok((config, state))
}

/// Build the reconstructed state from submitted/completed maps.
fn build_state(
    submitted: &BTreeMap<LogTaskId, TaskSubmitted>,
    completed: &BTreeMap<LogTaskId, TaskCompleted>,
    max_task_id: u32,
) -> ReconstructedState {
    // A task is "alive" if it was submitted but not completed,
    // OR if it completed with children that are alive.
    let alive = compute_alive_set(submitted, completed);

    let mut pending_tasks = Vec::new();
    let mut waiting_tasks = Vec::new();

    for task_id in &alive {
        let Some(sub) = submitted.get(task_id) else {
            continue;
        };

        if let Some(comp) = completed.get(task_id) {
            // Task completed but is alive → it has alive dependents → Waiting
            if let TaskOutcome::Success(_) = comp.outcome {
                // Count all alive tasks that depend on this task.
                // Uses parent_id for children/retries, and origin for finally tasks.
                let alive_count = count_alive_dependents(*task_id, submitted, &alive);
                if let Some(count) = NonZeroU16::new(alive_count) {
                    waiting_tasks.push(WaitingTask {
                        task_id: *task_id,
                        step: sub.step.clone(),
                        parent_id: sub.parent_id,
                        pending_children_count: count,
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

    // next_task_id: one past the highest seen ID, or 0 if no tasks
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
///
/// Counts tasks that have `parent_id` pointing to this task (covers children
/// and retry replacements), plus finally tasks (via origin) that don't already
/// have this task as `parent_id` (to avoid double-counting).
fn count_alive_dependents(
    task_id: LogTaskId,
    submitted: &BTreeMap<LogTaskId, TaskSubmitted>,
    alive: &BTreeSet<LogTaskId>,
) -> u16 {
    let mut count: u16 = 0;
    for s in submitted.values() {
        if !alive.contains(&s.task_id) {
            continue;
        }
        if s.parent_id == Some(task_id) {
            count = count.saturating_add(1);
        } else if matches!(&s.origin, TaskOrigin::Finally { finally_for } if *finally_for == task_id)
        {
            // Finally task not already counted via parent_id
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
///   (spawned child, retry replacement, or finally task)
fn compute_alive_set(
    submitted: &BTreeMap<LogTaskId, TaskSubmitted>,
    completed: &BTreeMap<LogTaskId, TaskCompleted>,
) -> BTreeSet<LogTaskId> {
    let mut alive = BTreeSet::new();

    // Seed: all submitted-but-not-completed tasks are alive
    for id in submitted.keys() {
        if !completed.contains_key(id) {
            alive.insert(*id);
        }
    }

    // Propagate upward: if a task is alive, its ancestors that are
    // waiting for it should also be alive.
    // We iterate until no new tasks are added (fixed point).
    loop {
        let mut changed = false;

        for (task_id, comp) in completed {
            if alive.contains(task_id) {
                continue; // Already marked alive
            }

            let TaskOutcome::Success(ref success) = comp.outcome else {
                continue; // Failed tasks don't wait for children
            };

            // Check if any spawned child is alive
            let has_alive_child = success.spawned_task_ids.iter().any(|id| alive.contains(id));

            // Check if any alive task has this task as parent_id.
            // This catches retry replacements: when a child fails and is retried,
            // the retry task has the same parent_id but is NOT in spawned_task_ids.
            let has_alive_dependent = submitted
                .values()
                .any(|s| s.parent_id == Some(*task_id) && alive.contains(&s.task_id));

            // Check if any finally task for this task is alive
            let has_alive_finally = submitted.values().any(|s| {
                matches!(&s.origin, TaskOrigin::Finally { finally_for } if *finally_for == *task_id)
                    && alive.contains(&s.task_id)
            });

            if has_alive_child || has_alive_dependent || has_alive_finally {
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

    // ==================== Helpers ====================

    fn config_entry() -> StateLogEntry {
        StateLogEntry::Config(StateLogConfig {
            config: json!({"steps": []}),
        })
    }

    fn submit(
        task_id: u32,
        step: &str,
        parent_id: Option<u32>,
        origin: TaskOrigin,
    ) -> StateLogEntry {
        StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: LogTaskId(task_id),
            step: StepName::new(step),
            value: StepInputValue(json!({"input": task_id})),
            parent_id: parent_id.map(LogTaskId),
            origin,
        })
    }

    fn submit_initial(task_id: u32, step: &str) -> StateLogEntry {
        submit(task_id, step, None, TaskOrigin::Initial)
    }

    fn submit_spawned(task_id: u32, step: &str, parent_id: u32) -> StateLogEntry {
        submit(task_id, step, Some(parent_id), TaskOrigin::Spawned)
    }

    fn complete_success(task_id: u32, spawned: &[u32]) -> StateLogEntry {
        StateLogEntry::TaskCompleted(TaskCompleted {
            task_id: LogTaskId(task_id),
            outcome: TaskOutcome::Success(TaskSuccess {
                spawned_task_ids: spawned.iter().map(|id| LogTaskId(*id)).collect(),
            }),
        })
    }

    fn complete_failed(task_id: u32, reason: FailureReason, retry: Option<u32>) -> StateLogEntry {
        StateLogEntry::TaskCompleted(TaskCompleted {
            task_id: LogTaskId(task_id),
            outcome: TaskOutcome::Failed(TaskFailed {
                reason,
                retry_task_id: retry.map(LogTaskId),
            }),
        })
    }

    fn submit_retry(
        task_id: u32,
        step: &str,
        parent_id: Option<u32>,
        replaces: u32,
    ) -> StateLogEntry {
        submit(
            task_id,
            step,
            parent_id,
            TaskOrigin::Retry {
                replaces: LogTaskId(replaces),
            },
        )
    }

    fn submit_finally(
        task_id: u32,
        step: &str,
        parent_id: Option<u32>,
        finally_for: u32,
    ) -> StateLogEntry {
        submit(
            task_id,
            step,
            parent_id,
            TaskOrigin::Finally {
                finally_for: LogTaskId(finally_for),
            },
        )
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
        let (_, state) =
            run_reconstruct(vec![config_entry(), submit_initial(0, "Analyze")]).unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(0));
        assert_eq!(state.pending_tasks[0].step, "Analyze");
        assert!(state.pending_tasks[0].parent_id.is_none());
        assert_eq!(state.pending_tasks[0].origin, TaskOrigin::Initial);
        assert!(state.waiting_tasks.is_empty());
        assert_eq!(state.next_task_id, 1);
    }

    #[test]
    fn reconstruct_single_task_completed_returns_empty() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "Analyze"),
            complete_success(0, &[]),
        ])
        .unwrap();

        assert!(state.pending_tasks.is_empty());
        assert!(state.waiting_tasks.is_empty());
    }

    #[test]
    fn reconstruct_submit_complete_submit_leaves_second_pending() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "Analyze"),
            complete_success(0, &[]),
            submit_initial(1, "Process"),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(1));
        assert_eq!(state.pending_tasks[0].step, "Process");
        assert_eq!(state.next_task_id, 2);
    }

    #[test]
    fn reconstruct_multiple_pending_tasks() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            submit_initial(1, "B"),
            submit_initial(2, "C"),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 3);
        assert_eq!(state.next_task_id, 3);
    }

    // ==================== Parent-Child Relationships ====================

    #[test]
    fn reconstruct_child_pending_parent_waiting() {
        // Parent completes, spawns child. Child still pending → parent is Waiting.
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "Analyze"),
            complete_success(0, &[1]),
            submit_spawned(1, "Process", 0),
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
            submit_initial(0, "Analyze"),
            complete_success(0, &[1, 2]),
            submit_spawned(1, "Process", 0),
            submit_spawned(2, "Process", 0),
            complete_success(1, &[]),
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
            submit_initial(0, "Analyze"),
            complete_success(0, &[1, 2]),
            submit_spawned(1, "Process", 0),
            submit_spawned(2, "Process", 0),
            complete_success(1, &[]),
            complete_success(2, &[]),
        ])
        .unwrap();

        assert!(state.pending_tasks.is_empty());
        assert!(state.waiting_tasks.is_empty());
    }

    #[test]
    fn reconstruct_grandchild_pending_sets_ancestor_waiting() {
        // Task 0 → spawns 1 → spawns 2 (still pending)
        // Both 0 and 1 should be Waiting
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_success(0, &[1]),
            submit_spawned(1, "B", 0),
            complete_success(1, &[2]),
            submit_spawned(2, "C", 1),
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

    #[test]
    fn reconstruct_preserves_parent_id_on_pending_tasks() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_success(0, &[1, 2]),
            submit_spawned(1, "B", 0),
            submit_spawned(2, "B", 0),
        ])
        .unwrap();

        for task in &state.pending_tasks {
            assert_eq!(task.parent_id, Some(LogTaskId(0)));
        }
    }

    // ==================== Retry Chains ====================

    #[test]
    fn reconstruct_failed_with_retry_only_retry_pending() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_failed(0, FailureReason::Timeout, Some(1)),
            submit_retry(1, "A", None, 0),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(1));
        assert_eq!(
            state.pending_tasks[0].origin,
            TaskOrigin::Retry {
                replaces: LogTaskId(0)
            }
        );
        assert!(state.waiting_tasks.is_empty());
    }

    #[test]
    fn reconstruct_failed_without_retry_task_dropped() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_failed(0, FailureReason::Timeout, None),
        ])
        .unwrap();

        assert!(state.pending_tasks.is_empty());
        assert!(state.waiting_tasks.is_empty());
    }

    #[test]
    fn reconstruct_retry_chain_only_final_pending() {
        // Task 0 fails → retry 1 fails → retry 2 (pending)
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_failed(0, FailureReason::Timeout, Some(1)),
            submit_retry(1, "A", None, 0),
            complete_failed(1, FailureReason::AgentLost, Some(2)),
            submit_retry(2, "A", None, 1),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(2));
    }

    #[test]
    fn reconstruct_retry_of_child_parent_still_waiting() {
        // Task 0 spawns child 1. Child 1 fails, retried as 2.
        // Parent 0 should be waiting for the retry.
        //
        // Key subtlety: task 2 (retry) has parent_id=0 but is NOT in
        // task 0's spawned_task_ids=[1]. We propagate alive-ness via
        // parent_id relationships, not just spawned_task_ids.
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_success(0, &[1]),
            submit_spawned(1, "B", 0),
            complete_failed(1, FailureReason::Timeout, Some(2)),
            submit_retry(2, "B", Some(0), 1),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(2));

        // Parent 0 is waiting because retry task 2 is alive and has parent_id=0
        assert_eq!(state.waiting_tasks.len(), 1);
        assert_eq!(state.waiting_tasks[0].task_id, LogTaskId(0));
        assert_eq!(state.waiting_tasks[0].pending_children_count.get(), 1);
    }

    // ==================== Finally Tasks ====================

    #[test]
    fn reconstruct_finally_pending_after_parent_complete() {
        // Task 0 completes with no children, finally task 1 is scheduled
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_success(0, &[]),
            submit_finally(1, "A", None, 0),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(1));
        assert_eq!(
            state.pending_tasks[0].origin,
            TaskOrigin::Finally {
                finally_for: LogTaskId(0)
            }
        );
    }

    #[test]
    fn reconstruct_finally_identifies_via_origin() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_success(0, &[1]),
            submit_spawned(1, "B", 0),
            complete_success(1, &[]),
            // After child 1 completes, task 0's children are all done.
            // Finally for task 0 is scheduled as sibling (parent=None since 0 has no parent)
            submit_finally(2, "A", None, 0),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(
            state.pending_tasks[0].origin,
            TaskOrigin::Finally {
                finally_for: LogTaskId(0)
            }
        );
    }

    #[test]
    fn reconstruct_finally_complete_all_done() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_success(0, &[]),
            submit_finally(1, "A", None, 0),
            complete_success(1, &[]),
        ])
        .unwrap();

        assert!(state.pending_tasks.is_empty());
        assert!(state.waiting_tasks.is_empty());
    }

    #[test]
    fn reconstruct_finally_with_alive_finally_parent_waiting() {
        // Task 0 (root, parent=None) spawns children 1,2.
        // Both complete. Finally task 3 scheduled for task 0 (parent=None, finally_for=0).
        // Task 3 is still pending → task 0 should be Waiting.
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_success(0, &[1, 2]),
            submit_spawned(1, "B", 0),
            submit_spawned(2, "B", 0),
            complete_success(1, &[]),
            complete_success(2, &[]),
            submit_finally(3, "A", None, 0),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(3));

        // Task 0 is waiting because its finally task is alive
        assert_eq!(state.waiting_tasks.len(), 1);
        assert_eq!(state.waiting_tasks[0].task_id, LogTaskId(0));
        assert_eq!(state.waiting_tasks[0].pending_children_count.get(), 1);
    }

    // ==================== Waiting State ====================

    #[test]
    fn reconstruct_waiting_has_correct_pending_count() {
        // 3 children, 1 complete → 2 alive
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_success(0, &[1, 2, 3]),
            submit_spawned(1, "B", 0),
            submit_spawned(2, "B", 0),
            submit_spawned(3, "B", 0),
            complete_success(1, &[]),
        ])
        .unwrap();

        assert_eq!(state.waiting_tasks.len(), 1);
        assert_eq!(state.waiting_tasks[0].pending_children_count.get(), 2);
    }

    #[test]
    fn reconstruct_waiting_task_not_in_pending() {
        // A waiting task should NOT appear in pending_tasks
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_success(0, &[1]),
            submit_spawned(1, "B", 0),
        ])
        .unwrap();

        let pending_ids: BTreeSet<_> = state.pending_tasks.iter().map(|t| t.task_id).collect();
        let waiting_ids: BTreeSet<_> = state.waiting_tasks.iter().map(|w| w.task_id).collect();

        // No overlap
        assert!(pending_ids.is_disjoint(&waiting_ids));
        // Task 0 is waiting, not pending
        assert!(waiting_ids.contains(&LogTaskId(0)));
        assert!(!pending_ids.contains(&LogTaskId(0)));
    }

    // ==================== Error Cases ====================

    #[test]
    fn reconstruct_duplicate_task_id_errors() {
        let result = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            submit_initial(0, "B"), // duplicate
        ]);
        assert!(matches!(result, Err(ReconstructError::DuplicateTaskId(_))));
    }

    #[test]
    fn reconstruct_complete_unknown_task_errors() {
        let result = run_reconstruct(vec![config_entry(), complete_success(99, &[])]);
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
        let result = run_reconstruct(vec![submit_initial(0, "A")]);
        assert!(matches!(result, Err(ReconstructError::FirstEntryNotConfig)));
    }

    #[test]
    fn reconstruct_already_completed_errors() {
        let result = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_success(0, &[]),
            complete_success(0, &[]), // duplicate
        ]);
        assert!(matches!(result, Err(ReconstructError::AlreadyCompleted(_))));
    }

    // ==================== Complex Scenarios ====================

    #[test]
    fn reconstruct_mixed_pending_waiting_done() {
        // Task 0: done (completed, no children)
        // Task 1: waiting (completed, child 3 pending)
        // Task 2: pending
        // Task 3: pending (child of 1)
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            submit_initial(1, "A"),
            submit_initial(2, "A"),
            complete_success(0, &[]),
            complete_success(1, &[3]),
            submit_spawned(3, "B", 1),
        ])
        .unwrap();

        let pending_ids: BTreeSet<_> = state.pending_tasks.iter().map(|t| t.task_id).collect();
        let waiting_ids: BTreeSet<_> = state.waiting_tasks.iter().map(|w| w.task_id).collect();

        assert_eq!(pending_ids.len(), 2);
        assert!(pending_ids.contains(&LogTaskId(2)));
        assert!(pending_ids.contains(&LogTaskId(3)));
        assert_eq!(waiting_ids.len(), 1);
        assert!(waiting_ids.contains(&LogTaskId(1)));
    }

    #[test]
    fn reconstruct_deep_nesting_five_levels() {
        // 0 → 1 → 2 → 3 → 4 (pending)
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_success(0, &[1]),
            submit_spawned(1, "A", 0),
            complete_success(1, &[2]),
            submit_spawned(2, "A", 1),
            complete_success(2, &[3]),
            submit_spawned(3, "A", 2),
            complete_success(3, &[4]),
            submit_spawned(4, "A", 3),
        ])
        .unwrap();

        assert_eq!(state.pending_tasks.len(), 1);
        assert_eq!(state.pending_tasks[0].task_id, LogTaskId(4));

        // All 4 ancestors are waiting
        assert_eq!(state.waiting_tasks.len(), 4);
        assert_eq!(state.next_task_id, 5);
    }

    #[test]
    fn reconstruct_wide_fanout_ten_children() {
        let mut entries = vec![
            config_entry(),
            submit_initial(0, "A"),
            complete_success(0, &[1, 2, 3, 4, 5, 6, 7, 8, 9, 10]),
        ];
        for i in 1..=10 {
            entries.push(submit_spawned(i, "B", 0));
        }
        // Complete half of them
        for i in 1..=5 {
            entries.push(complete_success(i, &[]));
        }

        let (_, state) = run_reconstruct(entries).unwrap();

        assert_eq!(state.pending_tasks.len(), 5); // 6-10
        assert_eq!(state.waiting_tasks.len(), 1); // task 0
        assert_eq!(state.waiting_tasks[0].pending_children_count.get(), 5);
    }

    #[test]
    fn reconstruct_interleaved_submits_and_completes() {
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit_initial(0, "A"),
            submit_initial(1, "A"),
            complete_success(0, &[2]),
            submit_spawned(2, "B", 0),
            complete_success(1, &[]),
            complete_success(2, &[]),
        ])
        .unwrap();

        // Everything done
        assert!(state.pending_tasks.is_empty());
        assert!(state.waiting_tasks.is_empty());
    }

    #[test]
    fn reconstruct_next_task_id_correct_with_gaps() {
        // Task IDs 0, 5, 10 - next should be 11
        let (_, state) = run_reconstruct(vec![
            config_entry(),
            submit(0, "A", None, TaskOrigin::Initial),
            submit(5, "A", None, TaskOrigin::Initial),
            submit(10, "A", None, TaskOrigin::Initial),
        ])
        .unwrap();

        assert_eq!(state.next_task_id, 11);
    }
}
