//! Detail pane widget — shows full context for the selected task.

use barnum_state::{FailureReason, TaskOrigin, TaskOutcome};
use barnum_types::LogTaskId;
use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph, Widget, Wrap},
};

use crate::app::{AppState, PanelFocus};
use crate::theme::{focused_border_style, unfocused_border_style, TaskStatus};

/// Widget for rendering the detail pane.
pub struct DetailWidget<'a> {
    app: &'a AppState,
    focused: bool,
}

impl<'a> DetailWidget<'a> {
    pub fn new(app: &'a AppState, focused: bool) -> Self {
        Self { app, focused }
    }

    fn build_content(&self) -> Vec<Line<'a>> {
        let Some(task_id) = self.app.selected_task else {
            return vec![Line::from(Span::styled(
                "Select a task to view details",
                Style::default().fg(Color::DarkGray),
            ))];
        };

        let Some(record) = self.app.tasks.get(&task_id) else {
            return vec![Line::from(Span::styled(
                "Task not found",
                Style::default().fg(Color::Red),
            ))];
        };

        let mut lines = Vec::new();

        // Header: Task t-{id} > {step} > {icon} {status}
        lines.push(Line::from(vec![
            Span::raw("Task "),
            Span::styled(format!("t-{}", record.id.0), Style::default().fg(Color::Cyan)),
            Span::raw(" > "),
            Span::styled(record.step.as_str().to_string(), Style::default().fg(Color::White)),
            Span::raw(" > "),
            Span::styled(
                format!("{} {}", record.status.icon(), record.status.label()),
                record.status.style(),
            ),
        ]));

        // Duration
        let duration = record
            .completed_at
            .map(|c| c.duration_since(record.submitted_at))
            .unwrap_or_else(|| record.submitted_at.elapsed());
        lines.push(Line::from(format!("Duration: {:.1}s", duration.as_secs_f64())));

        // Origin
        let origin_str = match &record.origin {
            TaskOrigin::Initial => "initial".to_string(),
            TaskOrigin::Spawned => "spawned".to_string(),
            TaskOrigin::Retry { replaces } => format!("retry (replaces t-{})", replaces.0),
            TaskOrigin::Finally { finally_for } => format!("finally (for t-{})", finally_for.0),
        };
        lines.push(Line::from(format!("Origin: {origin_str}")));

        // Parent chain
        let parent_chain = self.build_parent_chain(task_id);
        if !parent_chain.is_empty() {
            lines.push(Line::from(format!("Parent chain: {parent_chain}")));
        }

        // Children
        if !record.children.is_empty() {
            let children_str: Vec<String> = record
                .children
                .iter()
                .map(|cid| {
                    if let Some(child) = self.app.tasks.get(cid) {
                        format!("t-{} ({})", cid.0, child.step.as_str())
                    } else {
                        format!("t-{}", cid.0)
                    }
                })
                .collect();
            lines.push(Line::from(format!("Children: {}", children_str.join(", "))));
        }

        // Outcome details
        if let Some(outcome) = &record.outcome {
            lines.push(Line::raw(""));
            match outcome {
                TaskOutcome::Success(success) => {
                    if !success.spawned_task_ids.is_empty() {
                        let spawned: Vec<String> = success
                            .spawned_task_ids
                            .iter()
                            .map(|id| format!("t-{}", id.0))
                            .collect();
                        lines.push(Line::from(format!("Spawned: {}", spawned.join(", "))));
                    }
                }
                TaskOutcome::Failed(failed) => {
                    let failure_msg = match &failed.reason {
                        FailureReason::Timeout => "Timeout".to_string(),
                        FailureReason::AgentLost => "Agent lost".to_string(),
                        FailureReason::InvalidResponse { message } => {
                            format!("Invalid response: {message}")
                        }
                    };
                    lines.push(Line::from(Span::styled(
                        format!("Failure: {failure_msg}"),
                        Style::default().fg(Color::Red),
                    )));
                    if let Some(retry_id) = failed.retry_task_id {
                        lines.push(Line::from(format!("Retried as: t-{}", retry_id.0)));
                    }
                }
            }
        }

        // Value JSON
        lines.push(Line::raw(""));
        lines.push(Line::from(Span::styled(
            "Value:",
            Style::default().fg(Color::White),
        )));
        let pretty_json = serde_json::to_string_pretty(&record.value)
            .unwrap_or_else(|_| record.value.to_string());
        for json_line in pretty_json.lines() {
            lines.push(Line::from(Span::styled(
                json_line.to_string(),
                Style::default().fg(Color::DarkGray),
            )));
        }

        lines
    }

    fn build_parent_chain(&self, task_id: LogTaskId) -> String {
        let mut chain = Vec::new();
        let mut current = task_id;

        while let Some(record) = self.app.tasks.get(&current) {
            if let Some(parent_id) = record.parent_id {
                if let Some(parent) = self.app.tasks.get(&parent_id) {
                    chain.push(format!("t-{} ({})", parent_id.0, parent.step.as_str()));
                    current = parent_id;
                } else {
                    break;
                }
            } else {
                break;
            }
        }

        chain.join(" <- ")
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
            .title("Detail")
            .borders(Borders::ALL)
            .border_style(border_style);

        let content = self.build_content();
        let paragraph = Paragraph::new(content)
            .block(block)
            .wrap(Wrap { trim: false });

        paragraph.render(area, buf);
    }
}
