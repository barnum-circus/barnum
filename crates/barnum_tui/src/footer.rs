use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Widget;

use crate::app::{InputMode, PanelFocus};

/// Single-line footer bar showing context-sensitive keybindings.
pub struct FooterWidget {
    pub focus: PanelFocus,
    pub input_mode: InputMode,
}

impl FooterWidget {
    fn key_span(key: &str) -> Span<'_> {
        Span::styled(
            key,
            Style::default()
                .fg(Color::Cyan)
                .add_modifier(Modifier::BOLD),
        )
    }

    fn desc_span(desc: &str) -> Span<'_> {
        Span::styled(desc, Style::default().fg(Color::DarkGray))
    }

    fn separator() -> Span<'static> {
        Span::raw("  ")
    }

    fn panel_bindings(focus: PanelFocus) -> Vec<(&'static str, &'static str)> {
        match focus {
            PanelFocus::Graph => vec![
                ("\u{2190}\u{2192}", "pan"),
                ("\u{2191}\u{2193}", "select"),
                ("Enter", "filter tasks"),
                ("+/-", "zoom"),
            ],
            PanelFocus::TaskList => vec![
                ("j/k", "navigate"),
                ("Enter", "select"),
                ("s", "sort"),
                ("f", "filter"),
                ("/", "search"),
            ],
            PanelFocus::Detail => vec![
                ("j/k", "scroll"),
                ("y", "copy value"),
            ],
        }
    }

    fn common_bindings() -> Vec<(&'static str, &'static str)> {
        vec![
            ("Tab", "switch panel"),
            ("q", "quit"),
            ("?", "help"),
        ]
    }
}

impl Widget for FooterWidget {
    fn render(self, area: Rect, buf: &mut Buffer) {
        let panel = if self.input_mode == InputMode::Search {
            vec![
                ("Enter", "confirm"),
                ("Esc", "cancel"),
                ("Backspace", "delete"),
            ]
        } else {
            Self::panel_bindings(self.focus)
        };
        let common = Self::common_bindings();

        let mut spans: Vec<Span<'_>> = Vec::new();
        spans.push(Span::raw(" "));

        for (i, (key, desc)) in panel.iter().enumerate() {
            if i > 0 {
                spans.push(Self::separator());
            }
            spans.push(Self::key_span(key));
            spans.push(Span::raw(": "));
            spans.push(Self::desc_span(desc));
        }

        // Divider between panel-specific and common bindings
        spans.push(Span::raw("  \u{2502}  "));

        for (i, (key, desc)) in common.iter().enumerate() {
            if i > 0 {
                spans.push(Self::separator());
            }
            spans.push(Self::key_span(key));
            spans.push(Span::raw(": "));
            spans.push(Self::desc_span(desc));
        }

        let line = Line::from(spans);
        line.render(area, buf);
    }
}
