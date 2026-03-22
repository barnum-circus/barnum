use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::time::Instant;

use barnum_state::{TaskOrigin, TaskOutcome};
use barnum_types::{LogTaskId, StepInputValue, StepName};
use ratatui::widgets::ListState;

use crate::theme::TaskStatus;

// ---------------------------------------------------------------------------
// StatusCounts
// ---------------------------------------------------------------------------

/// Per-step task counts, broken down by status.
#[derive(Debug, Clone, Default)]
pub struct StatusCounts {
    pub pending: u32,
    pub in_flight: u32,
    pub completed: u32,
    pub failed: u32,
    pub retried: u32,
}

impl StatusCounts {
    pub fn increment(&mut self, status: TaskStatus) {
        match status {
            TaskStatus::Pending => self.pending += 1,
            TaskStatus::InFlight => self.in_flight += 1,
            TaskStatus::Completed => self.completed += 1,
            TaskStatus::Failed => self.failed += 1,
            TaskStatus::Retried => self.retried += 1,
        }
    }

    pub fn decrement(&mut self, status: TaskStatus) {
        match status {
            TaskStatus::Pending => self.pending = self.pending.saturating_sub(1),
            TaskStatus::InFlight => self.in_flight = self.in_flight.saturating_sub(1),
            TaskStatus::Completed => self.completed = self.completed.saturating_sub(1),
            TaskStatus::Failed => self.failed = self.failed.saturating_sub(1),
            TaskStatus::Retried => self.retried = self.retried.saturating_sub(1),
        }
    }

    pub fn total(&self) -> u32 {
        self.pending + self.in_flight + self.completed + self.failed + self.retried
    }
}

// ---------------------------------------------------------------------------
// TaskRecord
// ---------------------------------------------------------------------------

/// Full lifecycle record for a single task.
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub id: LogTaskId,
    pub step: StepName,
    pub status: TaskStatus,
    pub value: StepInputValue,
    pub parent_id: Option<LogTaskId>,
    pub children: Vec<LogTaskId>,
    pub submitted_at: Instant,
    pub completed_at: Option<Instant>,
    pub outcome: Option<TaskOutcome>,
    pub origin: TaskOrigin,
}

// ---------------------------------------------------------------------------
// PanelFocus
// ---------------------------------------------------------------------------

/// Which panel currently has keyboard focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PanelFocus {
    Graph,
    TaskList,
    Detail,
}

impl PanelFocus {
    pub fn next(self) -> Self {
        match self {
            Self::Graph => Self::TaskList,
            Self::TaskList => Self::Detail,
            Self::Detail => Self::Graph,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            Self::Graph => Self::Detail,
            Self::TaskList => Self::Graph,
            Self::Detail => Self::TaskList,
        }
    }
}

// ---------------------------------------------------------------------------
// InputMode
// ---------------------------------------------------------------------------

/// Whether the user is typing into a text field or using normal keybindings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum InputMode {
    /// Normal keybinding mode.
    Normal,
    /// Typing into the search bar.
    Search,
}

// ---------------------------------------------------------------------------
// RunStatus
// ---------------------------------------------------------------------------

/// Overall run status derived from aggregate task states.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RunStatus {
    Running,
    Completed,
    Failed,
    Waiting,
}

// ---------------------------------------------------------------------------
// Viewport / ZoomLevel
// ---------------------------------------------------------------------------

/// Zoom level for the DAG graph panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ZoomLevel {
    Full,
    Compact,
    Dot,
}

/// Scroll and zoom state for the graph panel.
#[derive(Debug, Clone)]
pub struct Viewport {
    pub scroll_x: i32,
    pub scroll_y: i32,
    pub zoom: ZoomLevel,
}

impl Default for Viewport {
    fn default() -> Self {
        Self {
            scroll_x: 0,
            scroll_y: 0,
            zoom: ZoomLevel::Full,
        }
    }
}

// ---------------------------------------------------------------------------
// SortColumn
// ---------------------------------------------------------------------------

/// Which column the task list is sorted by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SortColumn {
    Id,
    Status,
    Step,
    Duration,
    Parent,
}

impl SortColumn {
    pub fn next(self) -> Self {
        match self {
            Self::Id => Self::Status,
            Self::Status => Self::Step,
            Self::Step => Self::Duration,
            Self::Duration => Self::Parent,
            Self::Parent => Self::Id,
        }
    }
}

// ---------------------------------------------------------------------------
// AppState
// ---------------------------------------------------------------------------

/// Central application state for the TUI.
pub struct AppState {
    // -- Static --
    pub config_path: PathBuf,

    // -- Dynamic --
    pub tasks: BTreeMap<LogTaskId, TaskRecord>,
    pub step_counts: HashMap<StepName, StatusCounts>,
    pub run_status: RunStatus,
    pub start_time: Instant,
    pub total_events: u64,

    // -- UI --
    pub focus: PanelFocus,
    pub selected_step: Option<StepName>,
    pub selected_task: Option<LogTaskId>,
    pub task_list_state: ListState,
    pub graph_viewport: Viewport,
    pub status_filters: HashSet<TaskStatus>,
    pub input_mode: InputMode,
    pub search_query: String,
    pub sort_column: SortColumn,
    pub sort_reversed: bool,
}

impl AppState {
    pub fn new(config_path: PathBuf) -> Self {
        Self {
            config_path,
            tasks: BTreeMap::new(),
            step_counts: HashMap::new(),
            run_status: RunStatus::Waiting,
            start_time: Instant::now(),
            total_events: 0,
            focus: PanelFocus::Graph,
            selected_step: None,
            selected_task: None,
            task_list_state: ListState::default(),
            graph_viewport: Viewport::default(),
            status_filters: HashSet::new(),
            input_mode: InputMode::Normal,
            search_query: String::new(),
            sort_column: SortColumn::Id,
            sort_reversed: false,
        }
    }

    /// Apply a task-submitted event.
    pub fn apply_submitted(
        &mut self,
        task_id: LogTaskId,
        step: StepName,
        value: StepInputValue,
        parent_id: Option<LogTaskId>,
        origin: TaskOrigin,
    ) {
        let status = TaskStatus::InFlight;

        // Register as child of parent
        if let Some(pid) = parent_id {
            if let Some(parent) = self.tasks.get_mut(&pid) {
                parent.children.push(task_id);
            }
        }

        let record = TaskRecord {
            id: task_id,
            step: step.clone(),
            status,
            value,
            parent_id,
            children: Vec::new(),
            submitted_at: Instant::now(),
            completed_at: None,
            outcome: None,
            origin,
        };

        self.tasks.insert(task_id, record);
        self.step_counts
            .entry(step)
            .or_default()
            .increment(status);
        self.total_events += 1;
        self.update_run_status();
    }

    /// Apply a task-completed event.
    pub fn apply_completed(&mut self, task_id: LogTaskId, outcome: TaskOutcome) {
        let Some(record) = self.tasks.get_mut(&task_id) else {
            return;
        };

        let old_status = record.status;
        let new_status = match &outcome {
            TaskOutcome::Success(_) => TaskStatus::Completed,
            TaskOutcome::Failed(f) => {
                if f.retry_task_id.is_some() {
                    TaskStatus::Retried
                } else {
                    TaskStatus::Failed
                }
            }
        };

        record.status = new_status;
        record.completed_at = Some(Instant::now());
        record.outcome = Some(outcome);

        let step = record.step.clone();
        let counts = self.step_counts.entry(step).or_default();
        counts.decrement(old_status);
        counts.increment(new_status);

        self.total_events += 1;
        self.update_run_status();
    }

    /// Derive the overall run status from current task states.
    fn update_run_status(&mut self) {
        let mut has_in_flight = false;
        let mut has_pending = false;
        let mut has_failed = false;
        let mut all_terminal = true;

        for record in self.tasks.values() {
            match record.status {
                TaskStatus::InFlight => {
                    has_in_flight = true;
                    all_terminal = false;
                }
                TaskStatus::Pending => {
                    has_pending = true;
                    all_terminal = false;
                }
                TaskStatus::Failed => has_failed = true,
                TaskStatus::Completed | TaskStatus::Retried => {}
            }
        }

        self.run_status = if self.tasks.is_empty() {
            RunStatus::Waiting
        } else if has_in_flight || has_pending {
            RunStatus::Running
        } else if all_terminal && has_failed {
            RunStatus::Failed
        } else if all_terminal {
            RunStatus::Completed
        } else {
            RunStatus::Running
        };
    }

    /// Return task IDs matching current filters, search, and sort settings.
    pub fn visible_tasks(&self) -> Vec<LogTaskId> {
        let mut tasks: Vec<&TaskRecord> = self
            .tasks
            .values()
            .filter(|t| {
                // Step filter
                if let Some(ref step) = self.selected_step {
                    if t.step != *step {
                        return false;
                    }
                }
                // Status filter
                if !self.status_filters.is_empty() && !self.status_filters.contains(&t.status) {
                    return false;
                }
                // Search filter — matches against all visible columns.
                if !self.search_query.is_empty() {
                    let q = self.search_query.to_lowercase();
                    let id_str = format!("t-{:02}", t.id.0);
                    let step_str = t.step.as_str().to_lowercase();
                    let status_str = t.status.label().to_lowercase();
                    let value_str = t.value.0.to_string().to_lowercase();
                    if !id_str.contains(&q)
                        && !step_str.contains(&q)
                        && !status_str.contains(&q)
                        && !value_str.contains(&q)
                    {
                        return false;
                    }
                }
                true
            })
            .collect();

        // Sort
        tasks.sort_by(|a, b| {
            let cmp = match self.sort_column {
                SortColumn::Id => a.id.cmp(&b.id),
                SortColumn::Status => a.status.sort_priority().cmp(&b.status.sort_priority()),
                SortColumn::Step => a.step.as_str().cmp(b.step.as_str()),
                SortColumn::Duration => {
                    let dur_a = a
                        .completed_at
                        .unwrap_or_else(Instant::now)
                        .duration_since(a.submitted_at);
                    let dur_b = b
                        .completed_at
                        .unwrap_or_else(Instant::now)
                        .duration_since(b.submitted_at);
                    dur_a.cmp(&dur_b)
                }
                SortColumn::Parent => a.parent_id.cmp(&b.parent_id),
            };
            if self.sort_reversed {
                cmp.reverse()
            } else {
                cmp
            }
        });

        tasks.into_iter().map(|t| t.id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnum_state::{FailureReason, TaskFailed, TaskOrigin, TaskOutcome, TaskSuccess};
    use barnum_types::{LogTaskId, StepInputValue, StepName};
    use std::path::PathBuf;

    fn state() -> AppState {
        AppState::new(PathBuf::from("/tmp/test.json"))
    }

    fn val(s: &str) -> StepInputValue {
        StepInputValue(serde_json::json!(s))
    }

    fn success() -> TaskOutcome {
        TaskOutcome::Success(TaskSuccess {
            spawned_task_ids: vec![],
            finally_value: StepInputValue(serde_json::json!(null)),
        })
    }

    fn failed() -> TaskOutcome {
        TaskOutcome::Failed(TaskFailed {
            reason: FailureReason::Timeout,
            retry_task_id: None,
        })
    }

    fn retried(retry_id: u32) -> TaskOutcome {
        TaskOutcome::Failed(TaskFailed {
            reason: FailureReason::Timeout,
            retry_task_id: Some(LogTaskId(retry_id)),
        })
    }

    // -----------------------------------------------------------------------
    // apply_submitted
    // -----------------------------------------------------------------------

    #[test]
    fn apply_submitted_creates_record() {
        let mut app = state();
        app.apply_submitted(
            LogTaskId(1),
            StepName::new("Analyze"),
            val("hello"),
            None,
            TaskOrigin::Initial,
        );

        assert_eq!(app.tasks.len(), 1);
        let rec = &app.tasks[&LogTaskId(1)];
        assert_eq!(rec.id, LogTaskId(1));
        assert_eq!(rec.step.as_str(), "Analyze");
        assert_eq!(rec.status, TaskStatus::InFlight);
        assert_eq!(rec.parent_id, None);
        assert!(rec.children.is_empty());
        assert!(rec.completed_at.is_none());
        assert!(rec.outcome.is_none());
    }

    #[test]
    fn apply_submitted_increments_step_counts() {
        let mut app = state();
        let step = StepName::new("Build");
        app.apply_submitted(LogTaskId(1), step.clone(), val("a"), None, TaskOrigin::Initial);
        app.apply_submitted(LogTaskId(2), step.clone(), val("b"), None, TaskOrigin::Initial);

        let counts = &app.step_counts[&step];
        assert_eq!(counts.in_flight, 2);
        assert_eq!(counts.total(), 2);
    }

    #[test]
    fn apply_submitted_updates_run_status_to_running() {
        let mut app = state();
        assert_eq!(app.run_status, RunStatus::Waiting);

        app.apply_submitted(
            LogTaskId(1),
            StepName::new("X"),
            val("v"),
            None,
            TaskOrigin::Initial,
        );
        assert_eq!(app.run_status, RunStatus::Running);
    }

    #[test]
    fn apply_submitted_registers_child_on_parent() {
        let mut app = state();
        app.apply_submitted(LogTaskId(1), StepName::new("A"), val("v"), None, TaskOrigin::Initial);
        app.apply_submitted(
            LogTaskId(2),
            StepName::new("B"),
            val("v"),
            Some(LogTaskId(1)),
            TaskOrigin::Spawned,
        );

        assert_eq!(app.tasks[&LogTaskId(1)].children, vec![LogTaskId(2)]);
    }

    #[test]
    fn apply_submitted_increments_total_events() {
        let mut app = state();
        app.apply_submitted(LogTaskId(1), StepName::new("A"), val("v"), None, TaskOrigin::Initial);
        app.apply_submitted(LogTaskId(2), StepName::new("A"), val("v"), None, TaskOrigin::Initial);
        assert_eq!(app.total_events, 2);
    }

    // -----------------------------------------------------------------------
    // apply_completed
    // -----------------------------------------------------------------------

    #[test]
    fn apply_completed_success_transitions_status() {
        let mut app = state();
        app.apply_submitted(LogTaskId(1), StepName::new("S"), val("v"), None, TaskOrigin::Initial);
        app.apply_completed(LogTaskId(1), success());

        let rec = &app.tasks[&LogTaskId(1)];
        assert_eq!(rec.status, TaskStatus::Completed);
        assert!(rec.completed_at.is_some());
        assert!(rec.outcome.is_some());
    }

    #[test]
    fn apply_completed_failure_transitions_status() {
        let mut app = state();
        app.apply_submitted(LogTaskId(1), StepName::new("S"), val("v"), None, TaskOrigin::Initial);
        app.apply_completed(LogTaskId(1), failed());

        assert_eq!(app.tasks[&LogTaskId(1)].status, TaskStatus::Failed);
    }

    #[test]
    fn apply_completed_retry_transitions_to_retried() {
        let mut app = state();
        app.apply_submitted(LogTaskId(1), StepName::new("S"), val("v"), None, TaskOrigin::Initial);
        app.apply_completed(LogTaskId(1), retried(2));

        assert_eq!(app.tasks[&LogTaskId(1)].status, TaskStatus::Retried);
    }

    #[test]
    fn apply_completed_adjusts_step_counts() {
        let mut app = state();
        let step = StepName::new("S");
        app.apply_submitted(LogTaskId(1), step.clone(), val("v"), None, TaskOrigin::Initial);
        app.apply_submitted(LogTaskId(2), step.clone(), val("v"), None, TaskOrigin::Initial);

        // Before: 2 in_flight
        assert_eq!(app.step_counts[&step].in_flight, 2);

        app.apply_completed(LogTaskId(1), success());
        assert_eq!(app.step_counts[&step].in_flight, 1);
        assert_eq!(app.step_counts[&step].completed, 1);

        app.apply_completed(LogTaskId(2), failed());
        assert_eq!(app.step_counts[&step].in_flight, 0);
        assert_eq!(app.step_counts[&step].failed, 1);
    }

    #[test]
    fn apply_completed_unknown_task_is_noop() {
        let mut app = state();
        // Should not panic
        app.apply_completed(LogTaskId(999), success());
        assert_eq!(app.tasks.len(), 0);
    }

    // -----------------------------------------------------------------------
    // update_run_status derivation
    // -----------------------------------------------------------------------

    #[test]
    fn run_status_empty_is_waiting() {
        let app = state();
        assert_eq!(app.run_status, RunStatus::Waiting);
    }

    #[test]
    fn run_status_with_in_flight_is_running() {
        let mut app = state();
        app.apply_submitted(LogTaskId(1), StepName::new("S"), val("v"), None, TaskOrigin::Initial);
        assert_eq!(app.run_status, RunStatus::Running);
    }

    #[test]
    fn run_status_all_completed_is_completed() {
        let mut app = state();
        app.apply_submitted(LogTaskId(1), StepName::new("S"), val("v"), None, TaskOrigin::Initial);
        app.apply_submitted(LogTaskId(2), StepName::new("S"), val("v"), None, TaskOrigin::Initial);
        app.apply_completed(LogTaskId(1), success());
        app.apply_completed(LogTaskId(2), success());
        assert_eq!(app.run_status, RunStatus::Completed);
    }

    #[test]
    fn run_status_all_terminal_with_failure_is_failed() {
        let mut app = state();
        app.apply_submitted(LogTaskId(1), StepName::new("S"), val("v"), None, TaskOrigin::Initial);
        app.apply_submitted(LogTaskId(2), StepName::new("S"), val("v"), None, TaskOrigin::Initial);
        app.apply_completed(LogTaskId(1), success());
        app.apply_completed(LogTaskId(2), failed());
        assert_eq!(app.run_status, RunStatus::Failed);
    }

    #[test]
    fn run_status_retried_only_is_completed() {
        let mut app = state();
        app.apply_submitted(LogTaskId(1), StepName::new("S"), val("v"), None, TaskOrigin::Initial);
        app.apply_completed(LogTaskId(1), retried(2));
        app.apply_submitted(LogTaskId(2), StepName::new("S"), val("v"), None, TaskOrigin::Retry { replaces: LogTaskId(1) });
        app.apply_completed(LogTaskId(2), success());
        // Task 1 = Retried (terminal), Task 2 = Completed (terminal), no failures
        assert_eq!(app.run_status, RunStatus::Completed);
    }

    #[test]
    fn run_status_mixed_in_flight_and_failed_is_running() {
        let mut app = state();
        app.apply_submitted(LogTaskId(1), StepName::new("S"), val("v"), None, TaskOrigin::Initial);
        app.apply_submitted(LogTaskId(2), StepName::new("S"), val("v"), None, TaskOrigin::Initial);
        app.apply_completed(LogTaskId(1), failed());
        // Task 2 still in-flight
        assert_eq!(app.run_status, RunStatus::Running);
    }

    // -----------------------------------------------------------------------
    // visible_tasks — filtering
    // -----------------------------------------------------------------------

    fn populated_state() -> AppState {
        let mut app = state();
        app.apply_submitted(
            LogTaskId(1),
            StepName::new("Analyze"),
            val("alpha"),
            None,
            TaskOrigin::Initial,
        );
        app.apply_submitted(
            LogTaskId(2),
            StepName::new("Build"),
            val("beta"),
            None,
            TaskOrigin::Initial,
        );
        app.apply_submitted(
            LogTaskId(3),
            StepName::new("Analyze"),
            val("gamma"),
            None,
            TaskOrigin::Initial,
        );
        app.apply_completed(LogTaskId(2), success());
        app.apply_completed(LogTaskId(3), failed());
        // Task 1: Analyze, InFlight
        // Task 2: Build, Completed
        // Task 3: Analyze, Failed
        app
    }

    #[test]
    fn visible_tasks_no_filters_returns_all() {
        let app = populated_state();
        let ids = app.visible_tasks();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn visible_tasks_step_filter() {
        let mut app = populated_state();
        app.selected_step = Some(StepName::new("Analyze"));
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(1), LogTaskId(3)]);
    }

    #[test]
    fn visible_tasks_status_filter() {
        let mut app = populated_state();
        app.status_filters.insert(TaskStatus::Completed);
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(2)]);
    }

    #[test]
    fn visible_tasks_status_filter_multiple() {
        let mut app = populated_state();
        app.status_filters.insert(TaskStatus::InFlight);
        app.status_filters.insert(TaskStatus::Failed);
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(1), LogTaskId(3)]);
    }

    #[test]
    fn visible_tasks_search_by_id() {
        let mut app = populated_state();
        app.search_query = "t-01".to_string();
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(1)]);
    }

    #[test]
    fn visible_tasks_search_by_step_name() {
        let mut app = populated_state();
        app.search_query = "build".to_string();
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(2)]);
    }

    #[test]
    fn visible_tasks_search_by_status_label() {
        let mut app = populated_state();
        app.search_query = "in-flight".to_string();
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(1)]);
    }

    #[test]
    fn visible_tasks_search_by_value() {
        let mut app = populated_state();
        app.search_query = "gamma".to_string();
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(3)]);
    }

    #[test]
    fn visible_tasks_search_case_insensitive() {
        let mut app = populated_state();
        app.search_query = "ANALYZE".to_string();
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(1), LogTaskId(3)]);
    }

    #[test]
    fn visible_tasks_combined_step_and_status_filter() {
        let mut app = populated_state();
        app.selected_step = Some(StepName::new("Analyze"));
        app.status_filters.insert(TaskStatus::Failed);
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(3)]);
    }

    #[test]
    fn visible_tasks_combined_step_and_search() {
        let mut app = populated_state();
        app.selected_step = Some(StepName::new("Analyze"));
        app.search_query = "alpha".to_string();
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(1)]);
    }

    #[test]
    fn visible_tasks_combined_all_filters() {
        let mut app = populated_state();
        app.selected_step = Some(StepName::new("Analyze"));
        app.status_filters.insert(TaskStatus::InFlight);
        app.search_query = "alpha".to_string();
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(1)]);
    }

    // -----------------------------------------------------------------------
    // visible_tasks — sorting
    // -----------------------------------------------------------------------

    #[test]
    fn visible_tasks_sort_by_id() {
        let app = populated_state();
        // Default sort is by Id ascending
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(1), LogTaskId(2), LogTaskId(3)]);
    }

    #[test]
    fn visible_tasks_sort_by_id_reversed() {
        let mut app = populated_state();
        app.sort_reversed = true;
        let ids = app.visible_tasks();
        assert_eq!(ids, vec![LogTaskId(3), LogTaskId(2), LogTaskId(1)]);
    }

    #[test]
    fn visible_tasks_sort_by_status() {
        let mut app = populated_state();
        app.sort_column = SortColumn::Status;
        let ids = app.visible_tasks();
        // Priorities: InFlight=0, Failed=2, Completed=4
        assert_eq!(ids, vec![LogTaskId(1), LogTaskId(3), LogTaskId(2)]);
    }

    #[test]
    fn visible_tasks_sort_by_step() {
        let mut app = populated_state();
        app.sort_column = SortColumn::Step;
        let ids = app.visible_tasks();
        // "Analyze" < "Build", tasks 1 and 3 are Analyze, 2 is Build
        assert_eq!(ids[0], LogTaskId(1));
        assert_eq!(ids[2], LogTaskId(2));
    }

    #[test]
    fn visible_tasks_sort_by_parent() {
        let mut app = state();
        app.apply_submitted(LogTaskId(1), StepName::new("A"), val("v"), None, TaskOrigin::Initial);
        app.apply_submitted(
            LogTaskId(2),
            StepName::new("B"),
            val("v"),
            Some(LogTaskId(1)),
            TaskOrigin::Spawned,
        );
        app.apply_submitted(LogTaskId(3), StepName::new("C"), val("v"), None, TaskOrigin::Initial);

        app.sort_column = SortColumn::Parent;
        let ids = app.visible_tasks();
        // None < Some(1): tasks 1,3 have None, task 2 has Some(1)
        assert_eq!(*ids.last().unwrap(), LogTaskId(2));
    }

    #[test]
    fn visible_tasks_sort_by_duration() {
        let mut app = populated_state();
        app.sort_column = SortColumn::Duration;
        // All tasks submitted roughly at the same time; completed ones have short durations.
        // Just verify it doesn't panic and returns all tasks.
        let ids = app.visible_tasks();
        assert_eq!(ids.len(), 3);
    }

    #[test]
    fn visible_tasks_sort_reversed_by_status() {
        let mut app = populated_state();
        app.sort_column = SortColumn::Status;
        app.sort_reversed = true;
        let ids = app.visible_tasks();
        // Reversed: Completed=4, Failed=2, InFlight=0
        assert_eq!(ids, vec![LogTaskId(2), LogTaskId(3), LogTaskId(1)]);
    }

    // -----------------------------------------------------------------------
    // Edge cases
    // -----------------------------------------------------------------------

    #[test]
    fn visible_tasks_empty_state() {
        let app = state();
        assert!(app.visible_tasks().is_empty());
    }

    #[test]
    fn visible_tasks_no_matches() {
        let mut app = populated_state();
        app.search_query = "nonexistent_xyz".to_string();
        assert!(app.visible_tasks().is_empty());
    }

    #[test]
    fn visible_tasks_step_filter_no_matches() {
        let mut app = populated_state();
        app.selected_step = Some(StepName::new("NoSuchStep"));
        assert!(app.visible_tasks().is_empty());
    }

    #[test]
    fn status_counts_decrement_saturates_at_zero() {
        let mut counts = StatusCounts::default();
        counts.decrement(TaskStatus::InFlight);
        assert_eq!(counts.in_flight, 0);
    }
}
