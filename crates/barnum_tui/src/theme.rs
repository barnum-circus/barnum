use ratatui::style::{Color, Modifier, Style};

pub const COLOR_COMPLETED: Color = Color::Green;
pub const COLOR_IN_FLIGHT: Color = Color::Yellow;
pub const COLOR_PENDING: Color = Color::DarkGray;
pub const COLOR_FAILED: Color = Color::Red;
pub const COLOR_RETRIED: Color = Color::Magenta;

pub const ICON_COMPLETED: &str = "\u{2714}"; // checkmark
pub const ICON_IN_FLIGHT: &str = "\u{25CF}"; // filled circle
pub const ICON_PENDING: &str = "?";
pub const ICON_FAILED: &str = "\u{2718}"; // X mark
pub const ICON_RETRIED: &str = "\u{21BB}"; // clockwise arrow

pub fn header_style() -> Style {
    Style::default().fg(Color::White).bg(Color::DarkGray)
}

pub fn selected_style() -> Style {
    Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)
}

pub fn focused_border_style() -> Style {
    Style::default().fg(Color::Cyan)
}

pub fn unfocused_border_style() -> Style {
    Style::default().fg(Color::DarkGray)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum TaskStatus {
    Pending,
    InFlight,
    Completed,
    Failed,
    Retried,
}

impl TaskStatus {
    pub fn color(self) -> Color {
        match self {
            Self::Completed => COLOR_COMPLETED,
            Self::InFlight => COLOR_IN_FLIGHT,
            Self::Pending => COLOR_PENDING,
            Self::Failed => COLOR_FAILED,
            Self::Retried => COLOR_RETRIED,
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Self::Completed => ICON_COMPLETED,
            Self::InFlight => ICON_IN_FLIGHT,
            Self::Pending => ICON_PENDING,
            Self::Failed => ICON_FAILED,
            Self::Retried => ICON_RETRIED,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::InFlight => "in-flight",
            Self::Pending => "pending",
            Self::Failed => "failed",
            Self::Retried => "retried",
        }
    }

    pub fn sort_priority(self) -> u8 {
        match self {
            Self::InFlight => 0,
            Self::Pending => 1,
            Self::Failed => 2,
            Self::Retried => 3,
            Self::Completed => 4,
        }
    }
}
