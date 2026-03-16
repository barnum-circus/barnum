//! Theme constants for the TUI: colors, icons, and styles.

use ratatui::style::{Color, Modifier, Style};

/// Task status for display purposes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskStatus {
    Pending,
    InFlight,
    Completed,
    Failed,
    Retried,
}

impl TaskStatus {
    /// Color associated with this status.
    pub fn color(self) -> Color {
        match self {
            TaskStatus::Pending => Color::DarkGray,
            TaskStatus::InFlight => Color::Yellow,
            TaskStatus::Completed => Color::Green,
            TaskStatus::Failed => Color::Red,
            TaskStatus::Retried => Color::Magenta,
        }
    }

    /// Unicode icon for this status.
    pub fn icon(self) -> &'static str {
        match self {
            TaskStatus::Pending => "○",
            TaskStatus::InFlight => "●",
            TaskStatus::Completed => "✓",
            TaskStatus::Failed => "✗",
            TaskStatus::Retried => "↻",
        }
    }

    /// Human-readable label.
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Pending => "Pending",
            TaskStatus::InFlight => "In Flight",
            TaskStatus::Completed => "Completed",
            TaskStatus::Failed => "Failed",
            TaskStatus::Retried => "Retried",
        }
    }

    /// Sort priority (lower = first).
    pub fn sort_priority(self) -> u8 {
        match self {
            TaskStatus::Failed => 0,
            TaskStatus::InFlight => 1,
            TaskStatus::Pending => 2,
            TaskStatus::Retried => 3,
            TaskStatus::Completed => 4,
        }
    }

    /// Style with the status color applied.
    pub fn style(self) -> Style {
        Style::default().fg(self.color())
    }
}

/// Style for panel headers.
pub fn header_style() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

/// Style for selected items.
pub fn selected_style() -> Style {
    Style::default()
        .bg(Color::DarkGray)
        .add_modifier(Modifier::BOLD)
}

/// Border style for focused panels.
pub fn focused_border_style() -> Style {
    Style::default().fg(Color::Cyan)
}

/// Border style for unfocused panels.
pub fn unfocused_border_style() -> Style {
    Style::default().fg(Color::DarkGray)
}
