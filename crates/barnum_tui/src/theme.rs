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

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::style::Color;
    use std::collections::HashSet;

    const ALL_STATUSES: [TaskStatus; 5] = [
        TaskStatus::Pending,
        TaskStatus::InFlight,
        TaskStatus::Completed,
        TaskStatus::Failed,
        TaskStatus::Retried,
    ];

    // ── color ────────────────────────────────────────────────────

    #[test]
    fn color_completed_is_green() {
        assert_eq!(TaskStatus::Completed.color(), Color::Green);
    }

    #[test]
    fn color_in_flight_is_yellow() {
        assert_eq!(TaskStatus::InFlight.color(), Color::Yellow);
    }

    #[test]
    fn color_pending_is_dark_gray() {
        assert_eq!(TaskStatus::Pending.color(), Color::DarkGray);
    }

    #[test]
    fn color_failed_is_red() {
        assert_eq!(TaskStatus::Failed.color(), Color::Red);
    }

    #[test]
    fn color_retried_is_magenta() {
        assert_eq!(TaskStatus::Retried.color(), Color::Magenta);
    }

    // ── icon ─────────────────────────────────────────────────────

    #[test]
    fn icon_is_non_empty_for_all_statuses() {
        for status in ALL_STATUSES {
            assert!(
                !status.icon().is_empty(),
                "{:?} should have a non-empty icon",
                status
            );
        }
    }

    // ── label ────────────────────────────────────────────────────

    #[test]
    fn label_is_lowercase_for_all_statuses() {
        for status in ALL_STATUSES {
            let label = status.label();
            assert_eq!(
                label,
                label.to_lowercase(),
                "{:?} label should be lowercase",
                status
            );
        }
    }

    #[test]
    fn label_matches_expected() {
        assert_eq!(TaskStatus::Pending.label(), "pending");
        assert_eq!(TaskStatus::InFlight.label(), "in-flight");
        assert_eq!(TaskStatus::Completed.label(), "completed");
        assert_eq!(TaskStatus::Failed.label(), "failed");
        assert_eq!(TaskStatus::Retried.label(), "retried");
    }

    // ── sort_priority ────────────────────────────────────────────

    #[test]
    fn sort_priorities_are_distinct() {
        let priorities: HashSet<u8> = ALL_STATUSES.iter().map(|s| s.sort_priority()).collect();
        assert_eq!(priorities.len(), ALL_STATUSES.len());
    }

    #[test]
    fn sort_priority_ordering() {
        assert!(TaskStatus::InFlight.sort_priority() < TaskStatus::Pending.sort_priority());
        assert!(TaskStatus::Pending.sort_priority() < TaskStatus::Failed.sort_priority());
        assert!(TaskStatus::Failed.sort_priority() < TaskStatus::Retried.sort_priority());
        assert!(TaskStatus::Retried.sort_priority() < TaskStatus::Completed.sort_priority());
    }
}
