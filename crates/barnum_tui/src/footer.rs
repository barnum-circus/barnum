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
                ("Enter", "detail"),
                ("s/S", "sort/reverse"),
                ("f", "filter by step"),
                ("/", "search"),
                ("1-5", "status filter"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::{InputMode, PanelFocus};
    use ratatui::buffer::Buffer;
    use ratatui::layout::Rect;
    use ratatui::widgets::Widget;

    fn render_footer(focus: PanelFocus, input_mode: InputMode) -> String {
        let area = Rect::new(0, 0, 200, 1);
        let mut buf = Buffer::empty(area);
        let widget = FooterWidget { focus, input_mode };
        widget.render(area, &mut buf);
        (0..area.width)
            .map(|x| buf.cell((x, 0)).unwrap().symbol().to_string())
            .collect()
    }

    #[test]
    fn graph_normal_shows_pan_zoom_select() {
        let text = render_footer(PanelFocus::Graph, InputMode::Normal);
        assert!(text.contains("pan"), "should show 'pan': {text}");
        assert!(text.contains("zoom"), "should show 'zoom': {text}");
        assert!(text.contains("select"), "should show 'select': {text}");
    }

    #[test]
    fn task_list_normal_shows_navigate_detail_sort_filter_search_status_filter() {
        let text = render_footer(PanelFocus::TaskList, InputMode::Normal);
        assert!(text.contains("navigate"), "should show 'navigate': {text}");
        assert!(text.contains("detail"), "should show 'detail': {text}");
        assert!(text.contains("sort"), "should show 'sort': {text}");
        assert!(text.contains("filter"), "should show 'filter': {text}");
        assert!(text.contains("search"), "should show 'search': {text}");
        assert!(
            text.contains("status filter"),
            "should show 'status filter': {text}"
        );
    }

    #[test]
    fn detail_normal_shows_scroll_copy() {
        let text = render_footer(PanelFocus::Detail, InputMode::Normal);
        assert!(text.contains("scroll"), "should show 'scroll': {text}");
        assert!(text.contains("copy"), "should show 'copy': {text}");
    }

    #[test]
    fn search_mode_shows_confirm_cancel_delete() {
        let text = render_footer(PanelFocus::TaskList, InputMode::Search);
        assert!(text.contains("confirm"), "should show 'confirm': {text}");
        assert!(text.contains("cancel"), "should show 'cancel': {text}");
        assert!(text.contains("delete"), "should show 'delete': {text}");
    }

    #[test]
    fn all_modes_show_quit_and_switch_panel() {
        for focus in [PanelFocus::Graph, PanelFocus::TaskList, PanelFocus::Detail] {
            for mode in [InputMode::Normal, InputMode::Search] {
                let text = render_footer(focus, mode);
                assert!(
                    text.contains("quit"),
                    "should show 'quit' for {focus:?}/{mode:?}: {text}"
                );
                assert!(
                    text.contains("switch panel"),
                    "should show 'switch panel' for {focus:?}/{mode:?}: {text}"
                );
            }
        }
    }
}
