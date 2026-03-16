use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::{AppState, RunStatus};
use crate::theme;

/// Single-line header bar showing run identity, status, task count, and elapsed time.
pub struct HeaderWidget<'a> {
    pub app: &'a AppState,
}

impl<'a> HeaderWidget<'a> {
    fn status_icon(status: RunStatus) -> &'static str {
        match status {
            RunStatus::Running => "\u{25B6}",   // play triangle
            RunStatus::Completed => "\u{2714}",  // checkmark
            RunStatus::Failed => "\u{2718}",     // X mark
            RunStatus::Waiting => "\u{23F3}",    // hourglass
        }
    }

    fn status_label(status: RunStatus) -> &'static str {
        match status {
            RunStatus::Running => "running",
            RunStatus::Completed => "completed",
            RunStatus::Failed => "failed",
            RunStatus::Waiting => "waiting",
        }
    }

    fn status_color(status: RunStatus) -> Color {
        match status {
            RunStatus::Running => Color::Yellow,
            RunStatus::Completed => Color::Green,
            RunStatus::Failed => Color::Red,
            RunStatus::Waiting => Color::DarkGray,
        }
    }

    fn format_elapsed(app: &AppState) -> String {
        let elapsed = app.start_time.elapsed();
        let total_secs = elapsed.as_secs();
        let mins = total_secs / 60;
        let secs = total_secs % 60;
        format!("{mins:02}:{secs:02}")
    }

    fn total_tasks(app: &AppState) -> u32 {
        app.step_counts.values().map(|c| c.total()).sum()
    }
}

impl Widget for HeaderWidget<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let bg = theme::header_style();

        // Fill background
        for x in area.left()..area.right() {
            buf[(x, area.y)].set_style(bg);
        }

        let config_display = self.app.config_path.display().to_string();
        let status = self.app.run_status;
        let status_color = Self::status_color(status);
        let total = Self::total_tasks(self.app);
        let elapsed = Self::format_elapsed(self.app);

        let line = Line::from(vec![
            Span::styled(
                " barnum-tui",
                bg.add_modifier(Modifier::BOLD),
            ),
            Span::styled(" > ", bg),
            Span::styled(&config_display, bg),
            Span::styled("  ", bg),
            Span::styled(
                Self::status_icon(status),
                Style::default().fg(status_color).bg(bg.bg.unwrap_or(Color::Reset)),
            ),
            Span::styled(
                format!(" {}", Self::status_label(status)),
                Style::default().fg(status_color).bg(bg.bg.unwrap_or(Color::Reset)),
            ),
            Span::styled(
                format!("  {total} tasks"),
                bg,
            ),
            Span::styled(
                format!("  {elapsed}"),
                bg,
            ),
        ]);

        line.render(area, buf);
    }
}
