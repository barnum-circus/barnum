use std::time::Instant;

use barnum_state::{FailureReason, TaskFailed, TaskOrigin, TaskOutcome, TaskSuccess};
use barnum_types::LogTaskId;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use crate::app::{AppState, TaskRecord};
use crate::theme::{focused_border_style, unfocused_border_style};

/// Detail pane showing full information about the selected task.
pub struct DetailWidget<'a> {
    app: &'a AppState,
    focused: bool,
}

impl<'a> DetailWidget<'a> {
    pub fn new(app: &'a AppState, focused: bool) -> Self {
        Self { app, focused }
    }

    fn build_lines(&self) -> Vec<Line<'a>> {
        let Some(task_id) = self.app.selected_task else {
            return vec![Line::from(Span::styled(
                "Select a task to view details",
                Style::default().fg(Color::DarkGray),
            ))];
        };

        let Some(record) = self.app.tasks.get(&task_id) else {
            return vec![Line::from(Span::styled(
                "Task not found",
                Style::default().fg(Color::DarkGray),
            ))];
        };

        let mut lines = Vec::new();

        // 1. Header: "Task t-{id} > {step} > {icon} {status}"
        lines.push(self.header_line(record));
        lines.push(Line::from(""));

        // 2. Duration
        lines.push(self.duration_line(record));

        // 3. Origin
        lines.push(self.origin_line(record));

        // 4. Parent chain
        lines.push(self.parent_chain_line(record));

        // 5. Children
        if !record.children.is_empty() {
            lines.push(self.children_line(record));
        }

        lines.push(Line::from(""));

        // 6. Outcome
        if let Some(ref outcome) = record.outcome {
            self.push_outcome_lines(&mut lines, outcome);
        }

        lines.push(Line::from(""));

        // 7. Pretty-printed JSON value
        lines.push(Line::from(Span::styled(
            "Value:",
            Style::default().fg(Color::White),
        )));
        let json_str = serde_json::to_string_pretty(&record.value.0).unwrap_or_default();
        for json_line in json_str.lines() {
            lines.push(Line::from(Span::styled(
                json_line.to_string(),
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn header_line(&self, record: &TaskRecord) -> Line<'a> {
        let status = record.status;
        Line::from(vec![
            Span::styled("Task ", Style::default().fg(Color::White)),
            Span::styled(format!("t-{}", record.id.0), Style::default().fg(Color::Cyan)),
            Span::styled(" > ", Style::default().fg(Color::DarkGray)),
            Span::styled(record.step.to_string(), Style::default().fg(Color::White)),
            Span::styled(" > ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{} {}", status.icon(), status.label()),
                Style::default().fg(status.color()),
            ),
        ])
    }

    fn duration_line(&self, record: &TaskRecord) -> Line<'a> {
        let duration = record
            .completed_at
            .unwrap_or_else(Instant::now)
            .duration_since(record.submitted_at);
        let secs = duration.as_secs();
        let millis = duration.subsec_millis();

        let suffix = if record.completed_at.is_none() {
            " (running)"
        } else {
            ""
        };

        Line::from(vec![
            Span::styled("Duration: ", Style::default().fg(Color::DarkGray)),
            Span::styled(
                format!("{secs}.{millis:03}s{suffix}"),
                Style::default().fg(Color::White),
            ),
        ])
    }

    fn origin_line(&self, record: &TaskRecord) -> Line<'a> {
        let origin_text = match &record.origin {
            TaskOrigin::Initial => "initial".to_string(),
            TaskOrigin::Spawned => "spawned".to_string(),
            TaskOrigin::Retry { replaces } => format!("retry (replaces t-{})", replaces.0),
            TaskOrigin::Finally { finally_for } => {
                format!("finally (for t-{})", finally_for.0)
            }
        };

        Line::from(vec![
            Span::styled("Origin:   ", Style::default().fg(Color::DarkGray)),
            Span::styled(origin_text, Style::default().fg(Color::White)),
        ])
    }

    fn parent_chain_line(&self, record: &TaskRecord) -> Line<'a> {
        let chain = self.build_parent_chain(record);
        if chain.is_empty() {
            return Line::from(vec![
                Span::styled("Parents:  ", Style::default().fg(Color::DarkGray)),
                Span::styled("(root)", Style::default().fg(Color::DarkGray)),
            ]);
        }

        let mut spans = vec![Span::styled("Parents:  ", Style::default().fg(Color::DarkGray))];
        for (i, (id, step)) in chain.iter().enumerate() {
            if i > 0 {
                spans.push(Span::styled(" <- ", Style::default().fg(Color::DarkGray)));
            }
            spans.push(Span::styled(
                format!("t-{:02} ({})", id.0, step),
                Style::default().fg(Color::Cyan),
            ));
        }
        Line::from(spans)
    }

    fn build_parent_chain(&self, record: &TaskRecord) -> Vec<(LogTaskId, String)> {
        let mut chain = Vec::new();
        let mut current_id = record.parent_id;

        while let Some(pid) = current_id {
            if let Some(parent) = self.app.tasks.get(&pid) {
                chain.push((pid, parent.step.to_string()));
                current_id = parent.parent_id;
            } else {
                chain.push((pid, "?".to_string()));
                break;
            }
        }

        chain
    }

    fn children_line(&self, record: &TaskRecord) -> Line<'a> {
        let ids: Vec<String> = record.children.iter().map(|id| format!("t-{}", id.0)).collect();
        Line::from(vec![
            Span::styled("Children: ", Style::default().fg(Color::DarkGray)),
            Span::styled(ids.join(", "), Style::default().fg(Color::Cyan)),
        ])
    }

    fn push_outcome_lines(&self, lines: &mut Vec<Line<'a>>, outcome: &TaskOutcome) {
        match outcome {
            TaskOutcome::Success(TaskSuccess {
                spawned_task_ids,
                finally_value: _,
            }) => {
                if spawned_task_ids.is_empty() {
                    lines.push(Line::from(Span::styled(
                        "Outcome: success (no spawned tasks)",
                        Style::default().fg(Color::Green),
                    )));
                } else {
                    let ids: Vec<String> =
                        spawned_task_ids.iter().map(|id| format!("t-{}", id.0)).collect();
                    lines.push(Line::from(vec![
                        Span::styled("Outcome: ", Style::default().fg(Color::Green)),
                        Span::styled(
                            format!("success, spawned [{}]", ids.join(", ")),
                            Style::default().fg(Color::Green),
                        ),
                    ]));
                }
            }
            TaskOutcome::Failed(TaskFailed {
                reason,
                retry_task_id,
            }) => {
                let reason_text = match reason {
                    FailureReason::Timeout => "timeout".to_string(),
                    FailureReason::AgentLost => "agent lost".to_string(),
                    FailureReason::InvalidResponse { message } => {
                        format!("invalid response: {message}")
                    }
                };

                lines.push(Line::from(Span::styled(
                    format!("Outcome: failed - {reason_text}"),
                    Style::default().fg(Color::Red),
                )));

                if let Some(retry_id) = retry_task_id {
                    lines.push(Line::from(vec![
                        Span::styled("  Retried as: ", Style::default().fg(Color::DarkGray)),
                        Span::styled(
                            format!("t-{}", retry_id.0),
                            Style::default().fg(Color::Magenta),
                        ),
                    ]));
                } else {
                    lines.push(Line::from(Span::styled(
                        "  No retry (exhausted or disabled)",
                        Style::default().fg(Color::DarkGray),
                    )));
                }
            }
        }
    }
}

impl Widget for DetailWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let border_style = if self.focused {
            focused_border_style()
        } else {
            unfocused_border_style()
        };

        let block = Block::default()
            .title(" Detail ")
            .borders(Borders::ALL)
            .border_style(border_style);

        let lines = self.build_lines();

        let paragraph = Paragraph::new(lines)
            .block(block)
            .wrap(Wrap { trim: false });

        paragraph.render(area, buf);
    }
}
