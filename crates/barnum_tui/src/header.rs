//! Header bar widget showing config name, run status, task count, and elapsed time.

use std::time::Instant;

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::app::{AppState, RunStatus};

/// Header widget displaying run information.
pub struct HeaderWidget<'a> {
    app: &'a AppState,
}

impl<'a> HeaderWidget<'a> {
    pub fn new(app: &'a AppState) -> Self {
        Self { app }
    }

    fn run_status_icon(status: RunStatus) -> &'static str {
        match status {
            RunStatus::Waiting => "○",
            RunStatus::Running => "●",
            RunStatus::Completed => "✓",
            RunStatus::Failed => "✗",
        }
    }

    fn run_status_label(status: RunStatus) -> &'static str {
        match status {
            RunStatus::Waiting => "Waiting",
            RunStatus::Running => "Running",
            RunStatus::Completed => "Completed",
            RunStatus::Failed => "Failed",
        }
    }

    fn run_status_color(status: RunStatus) -> Color {
        match status {
            RunStatus::Waiting => Color::DarkGray,
            RunStatus::Running => Color::Yellow,
            RunStatus::Completed => Color::Green,
            RunStatus::Failed => Color::Red,
        }
    }

    fn format_elapsed(start_time: Option<Instant>) -> String {
        match start_time {
            Some(start) => {
                let elapsed = start.elapsed();
                let total_secs = elapsed.as_secs();
                let mins = total_secs / 60;
                let secs = total_secs % 60;
                format!("{:02}:{:02}", mins, secs)
            }
            None => "--:--".to_string(),
        }
    }

    fn total_task_count(&self) -> u32 {
        self.app.step_counts.values().map(|c| c.total()).sum()
    }
}

impl Widget for HeaderWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let header_bg = Style::default().bg(Color::DarkGray);

        // Fill entire line with background
        for x in area.x..area.x + area.width {
            buf[(x, area.y)].set_style(header_bg);
        }

        let status = self.app.run_status;
        let status_color = Self::run_status_color(status);

        let spans = vec![
            Span::styled(
                " barnum-tui",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" > "),
            Span::styled(&self.app.config_path, Style::default().fg(Color::White)),
            Span::raw("  "),
            Span::styled(
                Self::run_status_icon(status),
                Style::default().fg(status_color),
            ),
            Span::raw(" "),
            Span::styled(Self::run_status_label(status), Style::default().fg(status_color)),
            Span::raw("  "),
            Span::styled(
                format!("{} tasks", self.total_task_count()),
                Style::default().fg(Color::White),
            ),
            Span::raw("  "),
            Span::styled(
                Self::format_elapsed(self.app.start_time),
                Style::default().fg(Color::White),
            ),
            Span::raw(" "),
        ];

        let line = Line::from(spans);
        buf.set_line(area.x, area.y, &line, area.width);

        // Re-apply background to ensure it covers the whole line
        for x in area.x..area.x + area.width {
            buf[(x, area.y)].set_bg(Color::DarkGray);
        }
    }
}
