use ratatui::buffer::Buffer;
use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Cell, Row, StatefulWidget, Table, TableState};

use barnum_types::LogTaskId;

use crate::app::{AppState, InputMode};
use crate::theme;

/// Renders the task list as a styled `ratatui::Table`.
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

    /// Render with a `TableState` so ratatui can track the selected row.
    pub fn render_with_state(self, area: Rect, buf: &mut Buffer, state: &mut TableState) {
        let show_step_column = self.app.selected_step.is_none();

        let title = if self.app.input_mode == InputMode::Search {
            format!("Search: {}\u{2588}", self.app.search_query) // █ cursor
        } else {
            let base = if !self.app.search_query.is_empty() {
                format!("Tasks [/{}]", self.app.search_query)
            } else {
                match &self.app.selected_step {
                    Some(step) => format!("Tasks: {step}"),
                    None => "Tasks: All".to_string(),
                }
            };

            if self.app.status_filters.is_empty() {
                base
            } else {
                let mut labels: Vec<&str> = self
                    .app
                    .status_filters
                    .iter()
                    .map(|s| s.label())
                    .collect();
                labels.sort_unstable();
                format!("{base} [{}", labels.join(", ") + "]")
            }
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

        let header_cells = build_header_cells(show_step_column);
        let header = Row::new(header_cells)
            .style(
                Style::default()
                    .add_modifier(Modifier::BOLD)
                    .add_modifier(Modifier::UNDERLINED),
            )
            .height(1);

        let rows: Vec<Row> = self
            .tasks
            .iter()
            .filter_map(|id| self.app.tasks.get(id))
            .map(|record| {
                let status = record.status;
                let row_style = Style::default().fg(status.color());

                let duration_str = match record.status {
                    theme::TaskStatus::Pending => String::new(),
                    _ => {
                        let end = record
                            .completed_at
                            .unwrap_or_else(std::time::Instant::now);
                        format_duration(end.duration_since(record.submitted_at))
                    }
                };

                let value_str = format_value(&record.value.0);

                let mut cells = vec![
                    Cell::from(format!("t-{:02}", record.id.0)),
                    Cell::from(Line::from(vec![
                        Span::raw(status.icon()),
                        Span::raw(" "),
                        Span::raw(status.label()),
                    ])),
                ];

                if show_step_column {
                    cells.push(Cell::from(record.step.as_str().to_string()));
                }

                cells.push(Cell::from(duration_str));
                cells.push(Cell::from(value_str));

                Row::new(cells).style(row_style)
            })
            .collect();

        let widths = build_widths(show_step_column);

        let table = Table::new(rows, &widths)
            .header(header)
            .block(block)
            .row_highlight_style(theme::selected_style());

        StatefulWidget::render(table, area, buf, state);
    }
}

fn build_header_cells(show_step: bool) -> Vec<Cell<'static>> {
    let mut cells = vec![
        Cell::from("ID"),
        Cell::from("Status"),
    ];
    if show_step {
        cells.push(Cell::from("Step"));
    }
    cells.push(Cell::from("Duration"));
    cells.push(Cell::from("Value"));
    cells
}

fn build_widths(show_step: bool) -> Vec<Constraint> {
    let mut widths = vec![
        Constraint::Length(6),  // ID
        Constraint::Length(14), // Status (icon + space + label)
    ];
    if show_step {
        widths.push(Constraint::Length(12)); // Step
    }
    widths.push(Constraint::Length(8)); // Duration
    widths.push(Constraint::Fill(1));   // Value
    widths
}

fn format_duration(d: std::time::Duration) -> String {
    let secs = d.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        let m = secs / 60;
        let s = secs % 60;
        format!("{m}m{s:02}s")
    } else {
        let h = secs / 3600;
        let m = (secs % 3600) / 60;
        format!("{h}h{m:02}m")
    }
}

fn format_value(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    // ── format_duration ──────────────────────────────────────────

    #[test]
    fn format_duration_zero() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
    }

    #[test]
    fn format_duration_seconds_only() {
        assert_eq!(format_duration(Duration::from_secs(42)), "42s");
    }

    #[test]
    fn format_duration_just_under_a_minute() {
        assert_eq!(format_duration(Duration::from_secs(59)), "59s");
    }

    #[test]
    fn format_duration_exactly_60s() {
        assert_eq!(format_duration(Duration::from_secs(60)), "1m00s");
    }

    #[test]
    fn format_duration_minutes_and_seconds() {
        assert_eq!(format_duration(Duration::from_secs(65)), "1m05s");
    }

    #[test]
    fn format_duration_max_minutes() {
        assert_eq!(format_duration(Duration::from_secs(3599)), "59m59s");
    }

    #[test]
    fn format_duration_exactly_3600s() {
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h00m");
    }

    #[test]
    fn format_duration_hours_and_minutes() {
        assert_eq!(format_duration(Duration::from_secs(9000)), "2h30m");
    }

    #[test]
    fn format_duration_ignores_sub_second() {
        // 42.999s should still display as "42s"
        assert_eq!(format_duration(Duration::from_millis(42_999)), "42s");
    }

    // ── format_value ─────────────────────────────────────────────

    #[test]
    fn format_value_string() {
        let v = serde_json::Value::String("hello".into());
        assert_eq!(format_value(&v), "hello");
    }

    #[test]
    fn format_value_number() {
        let v = serde_json::json!(42);
        assert_eq!(format_value(&v), "42");
    }

    #[test]
    fn format_value_bool_true() {
        let v = serde_json::json!(true);
        assert_eq!(format_value(&v), "true");
    }

    #[test]
    fn format_value_bool_false() {
        let v = serde_json::json!(false);
        assert_eq!(format_value(&v), "false");
    }

    #[test]
    fn format_value_null() {
        let v = serde_json::Value::Null;
        assert_eq!(format_value(&v), "null");
    }

    #[test]
    fn format_value_object() {
        let v = serde_json::json!({"key": "val"});
        let result = format_value(&v);
        // serde_json::Value::to_string produces compact JSON
        assert!(result.contains("key"));
        assert!(result.contains("val"));
    }
}
