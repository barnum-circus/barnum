//! Keyboard event polling and dispatch.
//!
//! [`poll_event`] wraps crossterm's event poll with a timeout.
//! [`handle_key`] maps key presses to mutations on [`AppState`].

use std::time::Duration;

use crossterm::event::{self, Event, KeyCode, KeyEvent, KeyModifiers};

use crate::app::{AppState, InputMode, PanelFocus, ZoomLevel};
use crate::theme::TaskStatus;

/// Result of handling a key event.
pub enum EventResult {
    /// Continue the main loop.
    Continue,
    /// Exit the application.
    Quit,
}

/// Poll for a crossterm event with the given timeout.
///
/// Returns `None` if no event arrives within the timeout, or if the
/// event is not a key event.
pub fn poll_event(timeout: Duration) -> Option<Event> {
    if event::poll(timeout).ok()? {
        event::read().ok()
    } else {
        None
    }
}

/// Dispatch a key event to the appropriate handler based on focus.
pub fn handle_key(key: KeyEvent, app: &mut AppState) -> EventResult {
    // Ctrl-C always quits.
    if key.modifiers == KeyModifiers::CONTROL && key.code == KeyCode::Char('c') {
        return EventResult::Quit;
    }

    // While in search mode, route everything to the search handler.
    if app.input_mode == InputMode::Search {
        handle_search_input(key, app);
        return EventResult::Continue;
    }

    // Normal mode: global keys first.
    match (key.modifiers, key.code) {
        (_, KeyCode::Char('q')) => {
            return EventResult::Quit;
        }
        (_, KeyCode::Tab) => {
            app.focus = app.focus.next();
            return EventResult::Continue;
        }
        (_, KeyCode::BackTab) => {
            app.focus = app.focus.prev();
            return EventResult::Continue;
        }
        (_, KeyCode::Esc) => {
            app.search_query.clear();
            app.selected_step = None;
            app.selected_task = None;
            app.task_list_state.select(None);
            return EventResult::Continue;
        }
        _ => {}
    }

    match app.focus {
        PanelFocus::Graph => handle_graph_key(key, app),
        PanelFocus::TaskList => handle_task_list_key(key, app),
        PanelFocus::Detail => handle_detail_key(key, app),
    }

    EventResult::Continue
}

// ---------------------------------------------------------------------------
// Graph panel
// ---------------------------------------------------------------------------

fn handle_graph_key(key: KeyEvent, app: &mut AppState) {
    match key.code {
        KeyCode::Left | KeyCode::Char('h') => app.graph_viewport.scroll_x -= 2,
        KeyCode::Right | KeyCode::Char('l') => app.graph_viewport.scroll_x += 2,
        KeyCode::Up | KeyCode::Char('k') => app.graph_viewport.scroll_y -= 1,
        KeyCode::Down | KeyCode::Char('j') => app.graph_viewport.scroll_y += 1,
        KeyCode::Char('+') | KeyCode::Char('=') => {
            app.graph_viewport.zoom = match app.graph_viewport.zoom {
                ZoomLevel::Dot => ZoomLevel::Compact,
                ZoomLevel::Compact => ZoomLevel::Full,
                ZoomLevel::Full => ZoomLevel::Full, // already max
            };
        }
        KeyCode::Char('-') => {
            app.graph_viewport.zoom = match app.graph_viewport.zoom {
                ZoomLevel::Full => ZoomLevel::Compact,
                ZoomLevel::Compact => ZoomLevel::Dot,
                ZoomLevel::Dot => ZoomLevel::Dot, // already min
            };
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Task list panel
// ---------------------------------------------------------------------------

fn handle_task_list_key(key: KeyEvent, app: &mut AppState) {
    let visible = app.visible_tasks();
    let len = visible.len();

    match key.code {
        KeyCode::Char('j') | KeyCode::Down => {
            if len == 0 {
                return;
            }
            let next = match app.task_list_state.selected() {
                Some(i) if i + 1 < len => i + 1,
                Some(_) => len - 1,
                None => 0,
            };
            app.task_list_state.select(Some(next));
            app.selected_task = visible.get(next).copied();
        }
        KeyCode::Char('k') | KeyCode::Up => {
            if len == 0 {
                return;
            }
            let next = match app.task_list_state.selected() {
                Some(0) | None => 0,
                Some(i) => i - 1,
            };
            app.task_list_state.select(Some(next));
            app.selected_task = visible.get(next).copied();
        }
        KeyCode::Char('g') => {
            if len > 0 {
                app.task_list_state.select(Some(0));
                app.selected_task = visible.first().copied();
            }
        }
        KeyCode::Char('G') => {
            if len > 0 {
                app.task_list_state.select(Some(len - 1));
                app.selected_task = visible.last().copied();
            }
        }
        KeyCode::Char('f') => {
            // Toggle step filter: if a task is selected, filter to its step.
            // If already filtering, clear the filter.
            if app.selected_step.is_some() {
                app.selected_step = None;
            } else if let Some(task_id) = app.selected_task {
                if let Some(record) = app.tasks.get(&task_id) {
                    app.selected_step = Some(record.step.clone());
                }
            }
            app.task_list_state.select(None);
            app.selected_task = None;
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
        KeyCode::Char('/') => {
            app.input_mode = InputMode::Search;
        }
        KeyCode::Enter => {
            // Select current task and switch to detail panel.
            if let Some(idx) = app.task_list_state.selected() {
                if let Some(&task_id) = visible.get(idx) {
                    app.selected_task = Some(task_id);
                    app.focus = PanelFocus::Detail;
                }
            }
        }
        _ => {}
    }
}

// ---------------------------------------------------------------------------
// Search input
// ---------------------------------------------------------------------------

fn handle_search_input(key: KeyEvent, app: &mut AppState) {
    match key.code {
        KeyCode::Esc => {
            app.search_query.clear();
            app.input_mode = InputMode::Normal;
        }
        KeyCode::Enter => {
            // Confirm search and return to normal mode (query stays active).
            app.input_mode = InputMode::Normal;
        }
        KeyCode::Backspace => {
            app.search_query.pop();
            if app.search_query.is_empty() {
                app.input_mode = InputMode::Normal;
            }
        }
        KeyCode::Char(c) => {
            app.search_query.push(c);
        }
        _ => {}
    }
    // Reset selection when query changes so it doesn't point at a stale index.
    app.task_list_state.select(None);
    app.selected_task = None;
}

fn toggle_status_filter(app: &mut AppState, status: TaskStatus) {
    if app.status_filters.contains(&status) {
        app.status_filters.remove(&status);
    } else {
        app.status_filters.insert(status);
    }
}

// ---------------------------------------------------------------------------
// Detail panel
// ---------------------------------------------------------------------------

fn handle_detail_key(_key: KeyEvent, _app: &mut AppState) {
    // Clipboard copy (y) would use OSC 52 escape sequence.
    // Skipping for now as it requires base64 encoding.
}
