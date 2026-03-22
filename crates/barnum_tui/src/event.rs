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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::SortColumn;
    use barnum_state::TaskOrigin;
    use barnum_types::{LogTaskId, StepInputValue, StepName};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
    use std::path::PathBuf;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn ctrl(c: char) -> KeyEvent {
        KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL)
    }

    fn test_app() -> AppState {
        AppState::new(PathBuf::from("/tmp/test"))
    }

    fn test_app_with_tasks() -> AppState {
        let mut app = test_app();
        app.apply_submitted(
            LogTaskId(0),
            StepName::new("StepA"),
            StepInputValue(serde_json::json!("val0")),
            None,
            TaskOrigin::Initial,
        );
        app.apply_submitted(
            LogTaskId(1),
            StepName::new("StepB"),
            StepInputValue(serde_json::json!("val1")),
            None,
            TaskOrigin::Initial,
        );
        app.apply_submitted(
            LogTaskId(2),
            StepName::new("StepA"),
            StepInputValue(serde_json::json!("val2")),
            None,
            TaskOrigin::Initial,
        );
        app.focus = PanelFocus::TaskList;
        app
    }

    // =======================================================================
    // 1. Global keys (Normal mode)
    // =======================================================================

    #[test]
    fn global_q_quits() {
        let mut app = test_app();
        assert!(matches!(
            handle_key(key(KeyCode::Char('q')), &mut app),
            EventResult::Quit
        ));
    }

    #[test]
    fn global_ctrl_c_quits() {
        let mut app = test_app();
        assert!(matches!(
            handle_key(ctrl('c'), &mut app),
            EventResult::Quit
        ));
    }

    #[test]
    fn global_ctrl_c_quits_even_in_search_mode() {
        let mut app = test_app();
        app.input_mode = InputMode::Search;
        assert!(matches!(
            handle_key(ctrl('c'), &mut app),
            EventResult::Quit
        ));
    }

    #[test]
    fn global_tab_cycles_focus_forward() {
        let mut app = test_app();
        assert_eq!(app.focus, PanelFocus::Graph);

        handle_key(key(KeyCode::Tab), &mut app);
        assert_eq!(app.focus, PanelFocus::TaskList);

        handle_key(key(KeyCode::Tab), &mut app);
        assert_eq!(app.focus, PanelFocus::Detail);

        handle_key(key(KeyCode::Tab), &mut app);
        assert_eq!(app.focus, PanelFocus::Graph);
    }

    #[test]
    fn global_backtab_cycles_focus_reverse() {
        let mut app = test_app();
        assert_eq!(app.focus, PanelFocus::Graph);

        handle_key(key(KeyCode::BackTab), &mut app);
        assert_eq!(app.focus, PanelFocus::Detail);

        handle_key(key(KeyCode::BackTab), &mut app);
        assert_eq!(app.focus, PanelFocus::TaskList);

        handle_key(key(KeyCode::BackTab), &mut app);
        assert_eq!(app.focus, PanelFocus::Graph);
    }

    #[test]
    fn global_esc_clears_all_selections() {
        let mut app = test_app_with_tasks();
        app.search_query = "something".to_string();
        app.selected_step = Some(StepName::new("StepA"));
        app.selected_task = Some(LogTaskId(0));
        app.task_list_state.select(Some(1));

        let result = handle_key(key(KeyCode::Esc), &mut app);
        assert!(matches!(result, EventResult::Continue));
        assert!(app.search_query.is_empty());
        assert!(app.selected_step.is_none());
        assert!(app.selected_task.is_none());
        assert!(app.task_list_state.selected().is_none());
    }

    // =======================================================================
    // 2. Graph panel keys (focus = Graph)
    // =======================================================================

    #[test]
    fn graph_h_scrolls_left() {
        let mut app = test_app();
        app.focus = PanelFocus::Graph;
        app.graph_viewport.scroll_x = 10;

        handle_key(key(KeyCode::Char('h')), &mut app);
        assert_eq!(app.graph_viewport.scroll_x, 8);
    }

    #[test]
    fn graph_left_arrow_scrolls_left() {
        let mut app = test_app();
        app.graph_viewport.scroll_x = 10;

        handle_key(key(KeyCode::Left), &mut app);
        assert_eq!(app.graph_viewport.scroll_x, 8);
    }

    #[test]
    fn graph_l_scrolls_right() {
        let mut app = test_app();
        app.graph_viewport.scroll_x = 0;

        handle_key(key(KeyCode::Char('l')), &mut app);
        assert_eq!(app.graph_viewport.scroll_x, 2);
    }

    #[test]
    fn graph_right_arrow_scrolls_right() {
        let mut app = test_app();

        handle_key(key(KeyCode::Right), &mut app);
        assert_eq!(app.graph_viewport.scroll_x, 2);
    }

    #[test]
    fn graph_k_scrolls_up() {
        let mut app = test_app();
        app.graph_viewport.scroll_y = 5;

        handle_key(key(KeyCode::Char('k')), &mut app);
        assert_eq!(app.graph_viewport.scroll_y, 4);
    }

    #[test]
    fn graph_up_arrow_scrolls_up() {
        let mut app = test_app();
        app.graph_viewport.scroll_y = 5;

        handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(app.graph_viewport.scroll_y, 4);
    }

    #[test]
    fn graph_j_scrolls_down() {
        let mut app = test_app();
        app.graph_viewport.scroll_y = 0;

        handle_key(key(KeyCode::Char('j')), &mut app);
        assert_eq!(app.graph_viewport.scroll_y, 1);
    }

    #[test]
    fn graph_down_arrow_scrolls_down() {
        let mut app = test_app();

        handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(app.graph_viewport.scroll_y, 1);
    }

    #[test]
    fn graph_plus_zooms_in() {
        let mut app = test_app();
        app.graph_viewport.zoom = ZoomLevel::Dot;

        handle_key(key(KeyCode::Char('+')), &mut app);
        assert_eq!(app.graph_viewport.zoom, ZoomLevel::Compact);

        handle_key(key(KeyCode::Char('+')), &mut app);
        assert_eq!(app.graph_viewport.zoom, ZoomLevel::Full);
    }

    #[test]
    fn graph_equals_zooms_in() {
        let mut app = test_app();
        app.graph_viewport.zoom = ZoomLevel::Dot;

        handle_key(key(KeyCode::Char('=')), &mut app);
        assert_eq!(app.graph_viewport.zoom, ZoomLevel::Compact);
    }

    #[test]
    fn graph_zoom_in_clamps_at_full() {
        let mut app = test_app();
        app.graph_viewport.zoom = ZoomLevel::Full;

        handle_key(key(KeyCode::Char('+')), &mut app);
        assert_eq!(app.graph_viewport.zoom, ZoomLevel::Full);
    }

    #[test]
    fn graph_minus_zooms_out() {
        let mut app = test_app();
        app.graph_viewport.zoom = ZoomLevel::Full;

        handle_key(key(KeyCode::Char('-')), &mut app);
        assert_eq!(app.graph_viewport.zoom, ZoomLevel::Compact);

        handle_key(key(KeyCode::Char('-')), &mut app);
        assert_eq!(app.graph_viewport.zoom, ZoomLevel::Dot);
    }

    #[test]
    fn graph_zoom_out_clamps_at_dot() {
        let mut app = test_app();
        app.graph_viewport.zoom = ZoomLevel::Dot;

        handle_key(key(KeyCode::Char('-')), &mut app);
        assert_eq!(app.graph_viewport.zoom, ZoomLevel::Dot);
    }

    // =======================================================================
    // 3. Task list keys (focus = TaskList)
    // =======================================================================

    #[test]
    fn tasklist_j_selects_first_from_none() {
        let mut app = test_app_with_tasks();
        assert!(app.task_list_state.selected().is_none());

        handle_key(key(KeyCode::Char('j')), &mut app);
        assert_eq!(app.task_list_state.selected(), Some(0));
    }

    #[test]
    fn tasklist_j_increments_selection() {
        let mut app = test_app_with_tasks();
        app.task_list_state.select(Some(0));
        app.selected_task = Some(LogTaskId(0));

        handle_key(key(KeyCode::Char('j')), &mut app);
        assert_eq!(app.task_list_state.selected(), Some(1));
    }

    #[test]
    fn tasklist_down_increments_selection() {
        let mut app = test_app_with_tasks();
        app.task_list_state.select(Some(0));
        app.selected_task = Some(LogTaskId(0));

        handle_key(key(KeyCode::Down), &mut app);
        assert_eq!(app.task_list_state.selected(), Some(1));
    }

    #[test]
    fn tasklist_j_stops_at_end() {
        let mut app = test_app_with_tasks();
        app.task_list_state.select(Some(2));
        app.selected_task = Some(LogTaskId(2));

        handle_key(key(KeyCode::Char('j')), &mut app);
        assert_eq!(app.task_list_state.selected(), Some(2));
    }

    #[test]
    fn tasklist_k_decrements_selection() {
        let mut app = test_app_with_tasks();
        app.task_list_state.select(Some(2));
        app.selected_task = Some(LogTaskId(2));

        handle_key(key(KeyCode::Char('k')), &mut app);
        assert_eq!(app.task_list_state.selected(), Some(1));
    }

    #[test]
    fn tasklist_up_decrements_selection() {
        let mut app = test_app_with_tasks();
        app.task_list_state.select(Some(2));

        handle_key(key(KeyCode::Up), &mut app);
        assert_eq!(app.task_list_state.selected(), Some(1));
    }

    #[test]
    fn tasklist_k_stops_at_zero() {
        let mut app = test_app_with_tasks();
        app.task_list_state.select(Some(0));

        handle_key(key(KeyCode::Char('k')), &mut app);
        assert_eq!(app.task_list_state.selected(), Some(0));
    }

    #[test]
    fn tasklist_g_selects_first() {
        let mut app = test_app_with_tasks();
        app.task_list_state.select(Some(2));

        handle_key(key(KeyCode::Char('g')), &mut app);
        assert_eq!(app.task_list_state.selected(), Some(0));
        assert_eq!(app.selected_task, Some(LogTaskId(0)));
    }

    #[test]
    fn tasklist_g_upper_selects_last() {
        let mut app = test_app_with_tasks();
        app.task_list_state.select(Some(0));

        handle_key(key(KeyCode::Char('G')), &mut app);
        assert_eq!(app.task_list_state.selected(), Some(2));
        assert_eq!(app.selected_task, Some(LogTaskId(2)));
    }

    #[test]
    fn tasklist_enter_selects_task_and_switches_to_detail() {
        let mut app = test_app_with_tasks();
        app.task_list_state.select(Some(1));

        handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(app.selected_task, Some(LogTaskId(1)));
        assert_eq!(app.focus, PanelFocus::Detail);
    }

    #[test]
    fn tasklist_enter_without_selection_is_noop() {
        let mut app = test_app_with_tasks();
        // No selection
        handle_key(key(KeyCode::Enter), &mut app);
        assert!(app.selected_task.is_none());
        assert_eq!(app.focus, PanelFocus::TaskList);
    }

    #[test]
    fn tasklist_f_sets_step_filter_from_selected_task() {
        let mut app = test_app_with_tasks();
        app.selected_task = Some(LogTaskId(0));

        handle_key(key(KeyCode::Char('f')), &mut app);
        assert_eq!(app.selected_step, Some(StepName::new("StepA")));
        // Selection is cleared after filter toggle
        assert!(app.selected_task.is_none());
        assert!(app.task_list_state.selected().is_none());
    }

    #[test]
    fn tasklist_f_clears_step_filter_when_already_set() {
        let mut app = test_app_with_tasks();
        app.selected_step = Some(StepName::new("StepA"));

        handle_key(key(KeyCode::Char('f')), &mut app);
        assert!(app.selected_step.is_none());
    }

    #[test]
    fn tasklist_s_cycles_sort_column() {
        let mut app = test_app_with_tasks();
        assert_eq!(app.sort_column, SortColumn::Id);

        handle_key(key(KeyCode::Char('s')), &mut app);
        assert_eq!(app.sort_column, SortColumn::Status);

        handle_key(key(KeyCode::Char('s')), &mut app);
        assert_eq!(app.sort_column, SortColumn::Step);
    }

    #[test]
    fn tasklist_s_upper_toggles_sort_reversed() {
        let mut app = test_app_with_tasks();
        assert!(!app.sort_reversed);

        handle_key(key(KeyCode::Char('S')), &mut app);
        assert!(app.sort_reversed);

        handle_key(key(KeyCode::Char('S')), &mut app);
        assert!(!app.sort_reversed);
    }

    #[test]
    fn tasklist_1_toggles_pending_filter() {
        let mut app = test_app_with_tasks();
        assert!(!app.status_filters.contains(&TaskStatus::Pending));

        handle_key(key(KeyCode::Char('1')), &mut app);
        assert!(app.status_filters.contains(&TaskStatus::Pending));

        handle_key(key(KeyCode::Char('1')), &mut app);
        assert!(!app.status_filters.contains(&TaskStatus::Pending));
    }

    #[test]
    fn tasklist_2_toggles_inflight_filter() {
        let mut app = test_app_with_tasks();

        handle_key(key(KeyCode::Char('2')), &mut app);
        assert!(app.status_filters.contains(&TaskStatus::InFlight));
    }

    #[test]
    fn tasklist_3_toggles_completed_filter() {
        let mut app = test_app_with_tasks();

        handle_key(key(KeyCode::Char('3')), &mut app);
        assert!(app.status_filters.contains(&TaskStatus::Completed));
    }

    #[test]
    fn tasklist_4_toggles_failed_filter() {
        let mut app = test_app_with_tasks();

        handle_key(key(KeyCode::Char('4')), &mut app);
        assert!(app.status_filters.contains(&TaskStatus::Failed));
    }

    #[test]
    fn tasklist_5_toggles_retried_filter() {
        let mut app = test_app_with_tasks();

        handle_key(key(KeyCode::Char('5')), &mut app);
        assert!(app.status_filters.contains(&TaskStatus::Retried));
    }

    #[test]
    fn tasklist_slash_enters_search_mode() {
        let mut app = test_app_with_tasks();

        handle_key(key(KeyCode::Char('/')), &mut app);
        assert_eq!(app.input_mode, InputMode::Search);
    }

    // =======================================================================
    // 4. Search mode keys
    // =======================================================================

    #[test]
    fn search_typing_appends_to_query() {
        let mut app = test_app();
        app.input_mode = InputMode::Search;

        handle_key(key(KeyCode::Char('h')), &mut app);
        handle_key(key(KeyCode::Char('i')), &mut app);
        assert_eq!(app.search_query, "hi");
        assert_eq!(app.input_mode, InputMode::Search);
    }

    #[test]
    fn search_q_types_q_instead_of_quitting() {
        let mut app = test_app();
        app.input_mode = InputMode::Search;

        let result = handle_key(key(KeyCode::Char('q')), &mut app);
        assert!(matches!(result, EventResult::Continue));
        assert_eq!(app.search_query, "q");
    }

    #[test]
    fn search_backspace_pops_char() {
        let mut app = test_app();
        app.input_mode = InputMode::Search;
        app.search_query = "abc".to_string();

        handle_key(key(KeyCode::Backspace), &mut app);
        assert_eq!(app.search_query, "ab");
        assert_eq!(app.input_mode, InputMode::Search);
    }

    #[test]
    fn search_backspace_exits_when_empty() {
        let mut app = test_app();
        app.input_mode = InputMode::Search;
        app.search_query = "x".to_string();

        handle_key(key(KeyCode::Backspace), &mut app);
        assert!(app.search_query.is_empty());
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    #[test]
    fn search_enter_confirms_and_returns_to_normal() {
        let mut app = test_app();
        app.input_mode = InputMode::Search;
        app.search_query = "hello".to_string();

        handle_key(key(KeyCode::Enter), &mut app);
        assert_eq!(app.search_query, "hello"); // query preserved
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    #[test]
    fn search_esc_clears_query_and_returns_to_normal() {
        let mut app = test_app();
        app.input_mode = InputMode::Search;
        app.search_query = "hello".to_string();

        handle_key(key(KeyCode::Esc), &mut app);
        assert!(app.search_query.is_empty());
        assert_eq!(app.input_mode, InputMode::Normal);
    }

    #[test]
    fn search_resets_selection_on_input() {
        let mut app = test_app_with_tasks();
        app.input_mode = InputMode::Search;
        app.task_list_state.select(Some(1));
        app.selected_task = Some(LogTaskId(1));

        handle_key(key(KeyCode::Char('a')), &mut app);
        assert!(app.task_list_state.selected().is_none());
        assert!(app.selected_task.is_none());
    }

    // =======================================================================
    // 5. Detail panel keys
    // =======================================================================

    #[test]
    fn detail_random_keys_are_noop() {
        let mut app = test_app();
        app.focus = PanelFocus::Detail;

        let result = handle_key(key(KeyCode::Char('x')), &mut app);
        assert!(matches!(result, EventResult::Continue));

        let result = handle_key(key(KeyCode::Char('j')), &mut app);
        assert!(matches!(result, EventResult::Continue));

        let result = handle_key(key(KeyCode::Enter), &mut app);
        assert!(matches!(result, EventResult::Continue));
    }
}
