//! Central application state for the TUI.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Instant;

use barnum_state::{TaskOrigin, TaskOutcome};
use barnum_types::{LogTaskId, StepName};
use ratatui::widgets::ListState;

use crate::theme::TaskStatus;

/// Per-step task counts by status.
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

/// Full lifecycle record of a task.
#[derive(Debug, Clone)]
pub struct TaskRecord {
    pub id: LogTaskId,
    pub step: StepName,
    pub status: TaskStatus,
    pub value: serde_json::Value,
    pub parent_id: Option<LogTaskId>,
    pub children: Vec<LogTaskId>,
    pub submitted_at: Instant,
    pub completed_at: Option<Instant>,
    pub outcome: Option<TaskOutcome>,
    pub origin: TaskOrigin,
}

/// Which panel has focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PanelFocus {
    #[default]
    Graph,
    TaskList,
    Detail,
}

impl PanelFocus {
    pub fn next(self) -> Self {
        match self {
            PanelFocus::Graph => PanelFocus::TaskList,
            PanelFocus::TaskList => PanelFocus::Detail,
            PanelFocus::Detail => PanelFocus::Graph,
        }
    }

    pub fn prev(self) -> Self {
        match self {
            PanelFocus::Graph => PanelFocus::Detail,
            PanelFocus::TaskList => PanelFocus::Graph,
            PanelFocus::Detail => PanelFocus::TaskList,
        }
    }
}

/// Overall run status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RunStatus {
    #[default]
    Waiting,
    Running,
    Completed,
    Failed,
}

/// Graph viewport state.
#[derive(Debug, Clone, Default)]
pub struct Viewport {
    pub scroll_x: u16,
    pub scroll_y: u16,
    pub zoom: ZoomLevel,
}

/// Zoom level for the step graph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ZoomLevel {
    #[default]
    Full,
    Compact,
    Dot,
}

impl ZoomLevel {
    pub fn zoom_in(self) -> Self {
        match self {
            ZoomLevel::Dot => ZoomLevel::Compact,
            ZoomLevel::Compact => ZoomLevel::Full,
            ZoomLevel::Full => ZoomLevel::Full,
        }
    }

    pub fn zoom_out(self) -> Self {
        match self {
            ZoomLevel::Full => ZoomLevel::Compact,
            ZoomLevel::Compact => ZoomLevel::Dot,
            ZoomLevel::Dot => ZoomLevel::Dot,
        }
    }
}

/// Column to sort task list by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SortColumn {
    #[default]
    Id,
    Status,
    Step,
    Duration,
    Parent,
}

impl SortColumn {
    pub fn next(self) -> Self {
        match self {
            SortColumn::Id => SortColumn::Status,
            SortColumn::Status => SortColumn::Step,
            SortColumn::Step => SortColumn::Duration,
            SortColumn::Duration => SortColumn::Parent,
            SortColumn::Parent => SortColumn::Id,
        }
    }
}

/// Central application state.
#[derive(Debug)]
pub struct AppState {
    // Static
    pub config_path: String,

    // Dynamic
    pub tasks: BTreeMap<LogTaskId, TaskRecord>,
    pub step_counts: HashMap<StepName, StatusCounts>,
    pub run_status: RunStatus,
    pub start_time: Option<Instant>,
    pub total_events: u64,

    // UI
    pub focus: PanelFocus,
    pub selected_step: Option<StepName>,
    pub selected_task: Option<LogTaskId>,
    pub task_list_state: ListState,
    pub graph_viewport: Viewport,
    pub status_filters: HashSet<TaskStatus>,
    pub search_query: Option<String>,
    pub sort_column: SortColumn,
    pub sort_reversed: bool,
}

impl AppState {
    pub fn new(config_path: String) -> Self {
        Self {
            config_path,
            tasks: BTreeMap::new(),
            step_counts: HashMap::new(),
            run_status: RunStatus::Waiting,
            start_time: None,
            total_events: 0,
            focus: PanelFocus::default(),
            selected_step: None,
            selected_task: None,
            task_list_state: ListState::default(),
            graph_viewport: Viewport::default(),
            status_filters: HashSet::new(),
            search_query: None,
            sort_column: SortColumn::default(),
            sort_reversed: false,
        }
    }

    /// Apply a task submission event.
    pub fn apply_submitted(
        &mut self,
        task_id: LogTaskId,
        step: StepName,
        value: serde_json::Value,
        parent_id: Option<LogTaskId>,
        origin: TaskOrigin,
    ) {
        let record = TaskRecord {
            id: task_id,
            step: step.clone(),
            status: TaskStatus::InFlight,
            value,
            parent_id,
            children: Vec::new(),
            submitted_at: Instant::now(),
            completed_at: None,
            outcome: None,
            origin,
        };

        self.tasks.insert(task_id, record);

        // Update parent's children list
        if let Some(pid) = parent_id {
            if let Some(parent) = self.tasks.get_mut(&pid) {
                parent.children.push(task_id);
            }
        }

        // Update step counts
        self.step_counts
            .entry(step)
            .or_default()
            .increment(TaskStatus::InFlight);

        // Set run status to running
        if self.run_status == RunStatus::Waiting {
            self.run_status = RunStatus::Running;
            self.start_time = Some(Instant::now());
        }

        self.total_events += 1;
    }

    /// Apply a task completion event.
    pub fn apply_completed(&mut self, task_id: LogTaskId, outcome: TaskOutcome) {
        let Some(record) = self.tasks.get_mut(&task_id) else {
            return;
        };

        let old_status = record.status;
        let new_status = match &outcome {
            TaskOutcome::Success(_) => TaskStatus::Completed,
            TaskOutcome::Failed(failed) => {
                if failed.retry_task_id.is_some() {
                    TaskStatus::Retried
                } else {
                    TaskStatus::Failed
                }
            }
        };

        record.status = new_status;
        record.completed_at = Some(Instant::now());
        record.outcome = Some(outcome);

        // Update step counts
        if let Some(counts) = self.step_counts.get_mut(&record.step) {
            counts.decrement(old_status);
            counts.increment(new_status);
        }

        self.total_events += 1;
        self.update_run_status();
    }

    /// Derive run status from task states.
    pub fn update_run_status(&mut self) {
        let mut has_pending_or_inflight = false;
        let mut has_failed = false;

        for record in self.tasks.values() {
            match record.status {
                TaskStatus::Pending | TaskStatus::InFlight => {
                    has_pending_or_inflight = true;
                }
                TaskStatus::Failed => {
                    has_failed = true;
                }
                _ => {}
            }
        }

        self.run_status = if has_pending_or_inflight {
            RunStatus::Running
        } else if has_failed {
            RunStatus::Failed
        } else if self.tasks.is_empty() {
            RunStatus::Waiting
        } else {
            RunStatus::Completed
        };
    }

    /// Get visible tasks after applying filters and sorting.
    pub fn visible_tasks(&self) -> Vec<LogTaskId> {
        let mut tasks: Vec<_> = self
            .tasks
            .values()
            .filter(|t| {
                // Filter by selected step
                if let Some(ref step) = self.selected_step {
                    if &t.step != step {
                        return false;
                    }
                }

                // Filter by status
                if !self.status_filters.is_empty() && !self.status_filters.contains(&t.status) {
                    return false;
                }

                // Filter by search query
                if let Some(ref query) = self.search_query {
                    let query_lower = query.to_lowercase();
                    let matches_step = t.step.as_str().to_lowercase().contains(&query_lower);
                    let matches_id = t.id.0.to_string().contains(&query_lower);
                    let matches_value = t.value.to_string().to_lowercase().contains(&query_lower);
                    if !matches_step && !matches_id && !matches_value {
                        return false;
                    }
                }

                true
            })
            .collect();

        // Sort
        tasks.sort_by(|a, b| {
            let cmp = match self.sort_column {
                SortColumn::Id => a.id.0.cmp(&b.id.0),
                SortColumn::Status => a.status.sort_priority().cmp(&b.status.sort_priority()),
                SortColumn::Step => a.step.as_str().cmp(b.step.as_str()),
                SortColumn::Duration => {
                    let a_dur = a
                        .completed_at
                        .map(|c| c.duration_since(a.submitted_at))
                        .unwrap_or_else(|| a.submitted_at.elapsed());
                    let b_dur = b
                        .completed_at
                        .map(|c| c.duration_since(b.submitted_at))
                        .unwrap_or_else(|| b.submitted_at.elapsed());
                    a_dur.cmp(&b_dur)
                }
                SortColumn::Parent => a.parent_id.map(|p| p.0).cmp(&b.parent_id.map(|p| p.0)),
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
