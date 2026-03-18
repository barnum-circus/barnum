//! Keyboard event handling for the TUI.

use std::io::{self, Write};
use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

use crate::app::{AppState, PanelFocus, ZoomLevel};
use crate::theme::TaskStatus;

/// Result of handling an event.
pub enum EventResult {
    Continue,
    Quit,
}

/// Poll for a crossterm event with timeout.
pub fn poll_event(timeout: Duration) -> Option<Event> {
    match event::poll(timeout) {
        Ok(true) => event::read().ok(),
        _ => None,
    }
}

/// Handle a key event, dispatching to panel-specific handlers.
pub fn handle_key(key: KeyEvent, app: &mut AppState) -> EventResult {
    // Global keys
    match (key.code, key.modifiers) {
        (KeyCode::Char('q'), KeyModifiers::NONE) => return EventResult::Quit,
        (KeyCode::Char('c'), KeyModifiers::CONTROL) => return EventResult::Quit,
        (KeyCode::Tab, KeyModifiers::NONE) => {
            app.focus = app.focus.next();
            return EventResult::Continue;
        }
        (KeyCode::BackTab, KeyModifiers::SHIFT) | (KeyCode::BackTab, KeyModifiers::NONE) => {
            app.focus = app.focus.prev();
            return EventResult::Continue;
        }
        (KeyCode::Esc, KeyModifiers::NONE) => {
            app.search_query = None;
            app.selected_task = None;
            return EventResult::Continue;
        }
        _ => {}
    }

    // Panel-specific keys
    match app.focus {
        PanelFocus::Graph => handle_graph_key(key, app),
        PanelFocus::TaskList => handle_task_list_key(key, app),
        PanelFocus::Detail => handle_detail_key(key, app),
    }
}

fn handle_graph_key(key: KeyEvent, app: &mut AppState) -> EventResult {
    match key.code {
        KeyCode::Left => {
            app.graph_viewport.scroll_x = app.graph_viewport.scroll_x.saturating_sub(2);
        }
        KeyCode::Right => {
            app.graph_viewport.scroll_x = app.graph_viewport.scroll_x.saturating_add(2);
        }
        KeyCode::Up => {
            app.graph_viewport.scroll_y = app.graph_viewport.scroll_y.saturating_sub(1);
        }
        KeyCode::Down => {
            app.graph_viewport.scroll_y = app.graph_viewport.scroll_y.saturating_add(1);
        }
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.graph_viewport.zoom = match app.graph_viewport.zoom {
                ZoomLevel::Dot => ZoomLevel::Compact,
                ZoomLevel::Compact => ZoomLevel::Full,
                ZoomLevel::Full => ZoomLevel::Full,
            };
        }
        KeyCode::Char('-') => {
            app.graph_viewport.zoom = match app.graph_viewport.zoom {
                ZoomLevel::Full => ZoomLevel::Compact,
                ZoomLevel::Compact => ZoomLevel::Dot,
                ZoomLevel::Dot => ZoomLevel::Dot,
            };
        }
        _ => {}
    }
    EventResult::Continue
}

fn handle_task_list_key(key: KeyEvent, app: &mut AppState) -> EventResult {
    let visible_tasks = app.visible_tasks();
    let task_count = visible_tasks.len();

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if task_count > 0 {
                let current = app.task_list_state.selected().unwrap_or(0);
                let next = (current + 1).min(task_count.saturating_sub(1));
                app.task_list_state.select(Some(next));
                app.selected_task = visible_tasks.get(next).copied();
            }
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if task_count > 0 {
                let current = app.task_list_state.selected().unwrap_or(0);
                let next = current.saturating_sub(1);
                app.task_list_state.select(Some(next));
                app.selected_task = visible_tasks.get(next).copied();
            }
        }
        KeyCode::Char('g') => {
            if task_count > 0 {
                app.task_list_state.select(Some(0));
                app.selected_task = visible_tasks.first().copied();
            }
        }
        KeyCode::Char('G') => {
            if task_count > 0 {
                let last = task_count.saturating_sub(1);
                app.task_list_state.select(Some(last));
                app.selected_task = visible_tasks.get(last).copied();
            }
        }
        KeyCode::Char('s') => {
            app.sort_column = app.sort_column.next();
        }
        KeyCode::Char('S') => {
            app.sort_reversed = !app.sort_reversed;
        }
        KeyCode::Char('1') => toggle_status_filter(app, TaskStatus::Pending),
        KeyCode::Char('2') => toggle_status_filter(app, TaskStatus::InFlight),
        KeyCode::Char('3') => toggle_status_filter(app, TaskStatus::Completed),
        KeyCode::Char('4') => toggle_status_filter(app, TaskStatus::Failed),
        KeyCode::Char('5') => toggle_status_filter(app, TaskStatus::Retried),
        KeyCode::Enter => {
            if let Some(selected_idx) = app.task_list_state.selected() {
                app.selected_task = visible_tasks.get(selected_idx).copied();
                app.focus = PanelFocus::Detail;
            }
        }
        _ => {}
    }
    EventResult::Continue
}

fn toggle_status_filter(app: &mut AppState, status: TaskStatus) {
    if app.status_filters.contains(&status) {
        app.status_filters.remove(&status);
    } else {
        app.status_filters.insert(status);
    }
}

fn handle_detail_key(key: KeyEvent, app: &mut AppState) -> EventResult {
    match key.code {
        KeyCode::Char('y') => {
            if let Some(task_id) = app.selected_task {
                if let Some(record) = app.tasks.get(&task_id) {
                    copy_to_clipboard_osc52(&record.value.to_string());
                }
            }
        }
        _ => {}
    }
    EventResult::Continue
}

/// Copy text to clipboard using OSC 52 escape sequence.
fn copy_to_clipboard_osc52(text: &str) {
    use base64::Engine;
    let encoded = base64::engine::general_purpose::STANDARD.encode(text);
    // OSC 52: \x1b]52;c;<base64>\x07
    let _ = io::stdout().write_all(format!("\x1b]52;c;{}\x07", encoded).as_bytes());
    let _ = io::stdout().flush();
}
