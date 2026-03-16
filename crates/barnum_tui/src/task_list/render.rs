//! Task list table widget.

use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};

use barnum_types::LogTaskId;

use crate::app::AppState;
use crate::theme::{self, TaskStatus};

/// Widget for rendering the task list as a table.
pub struct TaskListWidget<'a> {
    tasks: &'a [LogTaskId],
    app: &'a AppState,
    focused: bool,
}

impl<'a> TaskListWidget<'a> {
    pub fn new(tasks: &'a [LogTaskId], app: &'a AppState, focused: bool) -> Self {
        Self {
            tasks,
            app,
            focused,
        }
    }

    /// Render the widget with mutable table state for selection tracking.
    pub fn render_with_state(self, area: Rect, buf: &mut Buffer, state: &mut TableState) {
        let show_step_column = self.app.selected_step.is_none();

        let title = match &self.app.selected_step {
            Some(step) => format!("Tasks: {}", step.as_str()),
            None => "Tasks: All".to_string(),
        };

        let border_style = if self.focused {
            theme::focused_border_style()
        } else {
            theme::unfocused_border_style()
        };

        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(border_style)
            .title(title);

        // Build header
        let header_style = Style::default()
            .add_modifier(Modifier::BOLD)
            .add_modifier(Modifier::UNDERLINED);

        let header_cells = if show_step_column {
            vec![
                Cell::from("ID"),
                Cell::from("Status"),
                Cell::from("Step"),
                Cell::from("Duration"),
                Cell::from("Value"),
            ]
        } else {
            vec![
                Cell::from("ID"),
                Cell::from("Status"),
                Cell::from("Duration"),
                Cell::from("Value"),
            ]
        };
        let header = Row::new(header_cells).style(header_style);

        // Build rows
        let rows: Vec<Row> = self
            .tasks
            .iter()
            .map(|task_id| {
                let record = self.app.tasks.get(task_id);
                match record {
                    Some(r) => self.build_row(r, show_step_column),
                    None => Row::new(vec![Cell::from("?")]),
                }
            })
            .collect();

        // Define column widths
        let widths = if show_step_column {
            vec![
                Constraint::Length(6),  // ID
                Constraint::Length(14), // Status
                Constraint::Length(12), // Step
                Constraint::Length(8),  // Duration
                Constraint::Min(10),    // Value (fill)
            ]
        } else {
            vec![
                Constraint::Length(6),  // ID
                Constraint::Length(14), // Status
                Constraint::Length(8),  // Duration
                Constraint::Min(10),    // Value (fill)
            ]
        };

        let table = Table::new(rows, widths)
            .header(header)
            .block(block)
            .highlight_style(theme::selected_style());

        ratatui::widgets::StatefulWidget::render(table, area, buf, state);
    }

    fn build_row(&self, record: &crate::app::TaskRecord, show_step_column: bool) -> Row<'static> {
        let id_cell = Cell::from(format!("t-{:02}", record.id.0));
        let status_cell = self.build_status_cell(record.status);
        let duration_cell = Cell::from(self.format_duration(record));
        let value_cell = Cell::from(self.truncate_value(&record.value));

        let row_style = record.status.style();

        if show_step_column {
            let step_cell = Cell::from(record.step.as_str().to_string());
            Row::new(vec![
                id_cell,
                status_cell,
                step_cell,
                duration_cell,
                value_cell,
            ])
            .style(row_style)
        } else {
            Row::new(vec![id_cell, status_cell, duration_cell, value_cell]).style(row_style)
        }
    }

    fn build_status_cell(&self, status: TaskStatus) -> Cell<'static> {
        let text = format!("{} {}", status.icon(), status.label());
        Cell::from(text).style(status.style())
    }

    fn format_duration(&self, record: &crate::app::TaskRecord) -> String {
        let duration = match record.completed_at {
            Some(completed) => completed.duration_since(record.submitted_at),
            None => record.submitted_at.elapsed(),
        };

        let secs = duration.as_secs();
        if secs < 60 {
            format!("{}s", secs)
        } else if secs < 3600 {
            let mins = secs / 60;
            let remaining_secs = secs % 60;
            format!("{}m{:02}s", mins, remaining_secs)
        } else {
            let hours = secs / 3600;
            let mins = (secs % 3600) / 60;
            format!("{}h{:02}m", hours, mins)
        }
    }

    fn truncate_value(&self, value: &serde_json::Value) -> String {
        let s = value.to_string();
        if s.len() > 40 {
            format!("{}...", &s[..40])
        } else {
            s
        }
    }
}
