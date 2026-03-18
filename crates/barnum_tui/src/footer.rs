//! Footer bar widget showing contextual keybinding hints.

use ratatui::{
    buffer::Buffer,
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Widget,
};

use crate::app::PanelFocus;

/// Footer widget displaying keybinding hints.
pub struct FooterWidget {
    focus: PanelFocus,
}

impl FooterWidget {
    pub fn new(focus: PanelFocus) -> Self {
        Self { focus }
    }

    fn key_style() -> Style {
        Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD)
    }

    fn desc_style() -> Style {
        Style::default().fg(Color::DarkGray)
    }

    fn hint(key: &str, desc: &str) -> Vec<Span<'static>> {
        vec![
            Span::styled(key.to_string(), Self::key_style()),
            Span::styled(format!(": {}  ", desc), Self::desc_style()),
        ]
    }

    fn graph_hints() -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        spans.extend(Self::hint("\u{2190}\u{2192}", "pan"));
        spans.extend(Self::hint("\u{2191}\u{2193}", "select"));
        spans.extend(Self::hint("Enter", "filter tasks"));
        spans.extend(Self::hint("+/-", "zoom"));
        spans.extend(Self::hint("Tab", "switch panel"));
        spans.extend(Self::hint("q", "quit"));
        spans.extend(Self::hint("?", "help"));
        spans
    }

    fn task_list_hints() -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        spans.extend(Self::hint("j/k", "navigate"));
        spans.extend(Self::hint("Enter", "select"));
        spans.extend(Self::hint("s", "sort"));
        spans.extend(Self::hint("f", "filter"));
        spans.extend(Self::hint("/", "search"));
        spans.extend(Self::hint("Tab", "switch panel"));
        spans.extend(Self::hint("q", "quit"));
        spans.extend(Self::hint("?", "help"));
        spans
    }

    fn detail_hints() -> Vec<Span<'static>> {
        let mut spans = Vec::new();
        spans.extend(Self::hint("j/k", "scroll"));
        spans.extend(Self::hint("y", "copy value"));
        spans.extend(Self::hint("Tab", "switch panel"));
        spans.extend(Self::hint("q", "quit"));
        spans.extend(Self::hint("?", "help"));
        spans
    }
}

impl Widget for FooterWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let bg_style = Style::default().bg(Color::DarkGray);

        // Fill entire line with background
        for x in area.x..area.x + area.width {
            buf[(x, area.y)].set_style(bg_style);
        }

        let mut spans = vec![Span::raw(" ")];
        spans.extend(match self.focus {
            PanelFocus::Graph => Self::graph_hints(),
            PanelFocus::TaskList => Self::task_list_hints(),
            PanelFocus::Detail => Self::detail_hints(),
        });

        let line = Line::from(spans);
        buf.set_line(area.x, area.y, &line, area.width);

        // Re-apply background
        for x in area.x..area.x + area.width {
            buf[(x, area.y)].set_bg(Color::DarkGray);
        }
    }
}
