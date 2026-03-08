//! Finally hook tracking and execution.

use std::collections::{HashMap, HashSet};
use tracing::{info, warn};

use crate::types::{HookScript, LogTaskId};
use crate::value_schema::Task;

use super::shell::run_shell_command;

pub struct FinallyState {
    pub pending_count: usize,
    pub original_value: serde_json::Value,
    pub finally_command: HookScript,
}

pub struct FinallyTracker {
    tracking: HashMap<LogTaskId, FinallyState>,
    /// Origins whose finally hooks have already run.
    /// Used to detect orphaned notifications (bug: notifying an origin twice).
    completed: HashSet<LogTaskId>,
}

impl FinallyTracker {
    pub fn new() -> Self {
        Self {
            tracking: HashMap::new(),
            completed: HashSet::new(),
        }
    }

    pub fn start_tracking(
        &mut self,
        task_id: LogTaskId,
        pending_count: usize,
        value: serde_json::Value,
        finally_command: HookScript,
    ) {
        self.tracking.insert(
            task_id,
            FinallyState {
                pending_count,
                original_value: value,
                finally_command,
            },
        );
    }

    /// Record that a descendant of `origin_id` has completed.
    ///
    /// Returns `Some(FinallyState)` when all descendants are done and the
    /// finally hook is ready to run. Returns `None` if descendants remain
    /// or if `origin_id` has no finally tracking (no-op for tasks without finally hooks).
    ///
    /// # Panics
    ///
    /// Panics if called for an origin whose finally hook has already run.
    /// This indicates a bug: either a task was counted twice, or a retry was
    /// incorrectly treated as a separate descendant.
    pub fn record_descendant_done(&mut self, origin_id: LogTaskId) -> Option<FinallyState> {
        // Detect orphaned notifications - a bug where we notify an origin twice
        assert!(
            !self.completed.contains(&origin_id),
            "BUG: orphaned notification for origin {origin_id:?} - finally already ran. \
             This usually means a retry was incorrectly treated as a descendant."
        );

        let ready_for_finally = if let Some(state) = self.tracking.get_mut(&origin_id) {
            state.pending_count = state.pending_count.saturating_sub(1);
            state.pending_count == 0
        } else {
            // Not tracked - origin has no finally hook, this is expected
            return None;
        };

        if ready_for_finally {
            let state = self.tracking.remove(&origin_id);
            if state.is_some() {
                self.completed.insert(origin_id);
            }
            state
        } else {
            None
        }
    }
}

pub fn run_finally_hook(state: &FinallyState) -> Vec<Task> {
    run_finally_hook_direct(&state.finally_command, &state.original_value)
}

/// Run a finally hook directly without going through the tracker.
/// Used when a task with a finally hook spawns no children (runs immediately).
#[expect(clippy::expect_used, reason = "serde_json::Value always serializes")]
pub fn run_finally_hook_direct(
    finally_command: &HookScript,
    value: &serde_json::Value,
) -> Vec<Task> {
    info!(command = %finally_command, "running finally hook");

    let input_json =
        serde_json::to_string(value).expect("[P018] serde_json::Value should always serialize");

    match run_shell_command(finally_command.as_str(), &input_json, None) {
        Ok(stdout) => match serde_json::from_str::<Vec<Task>>(&stdout) {
            Ok(tasks) => {
                info!(count = tasks.len(), "finally hook spawned tasks");
                tasks
            }
            Err(e) => {
                warn!(error = %e, "finally hook output is not valid JSON (ignored)");
                vec![]
            }
        },
        Err(e) => {
            warn!(error = %e, "finally hook failed (ignored)");
            vec![]
        }
    }
}
