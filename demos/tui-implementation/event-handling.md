# EventHandling

Implement keyboard event handling — poll for crossterm events and dispatch to panel-specific handlers that update AppState.

## Input

```json
{"workspace_root": "/path/to/barnum-worktree"}
```

## Context

All widget modules exist. This module connects keyboard input to AppState mutations.

Read:
- `{workspace_root}/crates/barnum_tui/src/app.rs` — AppState, PanelFocus, SortColumn, ZoomLevel, TaskStatus

## Instructions

### 1. Create `crates/barnum_tui/src/event.rs`

**Public API:**

```rust
pub enum EventResult {
    Continue,
    Quit,
}

pub fn poll_event(timeout: Duration) -> Option<Event>;
pub fn handle_key(key: KeyEvent, app: &mut AppState) -> EventResult;
```

**`poll_event`:** Wraps `crossterm::event::poll` + `read`. Returns `None` on timeout.

**`handle_key`:** Dispatches to panel-specific handlers.

**Global keys (all panels):**
- `q` → Quit
- `Ctrl-c` → Quit
- `Tab` → `app.focus.next()`
- `Shift-Tab` → `app.focus.prev()`
- `Esc` → Clear search_query and selected_task

**Graph panel keys:**
- `Left/Right` → scroll viewport x by 2
- `Up/Down` → scroll viewport y by 1
- `+`/`=` → zoom in: Dot→Compact→Full
- `-` → zoom out: Full→Compact→Dot

**TaskList panel keys:**
- `j`/`Down` → move selection down (clamped to list length)
- `k`/`Up` → move selection up (clamped to 0)
- `g` → select first task
- `G` → select last task
- `s` → cycle sort column
- `S` → toggle sort_reversed
- `1`-`5` → toggle status filters (1=Pending, 2=InFlight, 3=Completed, 4=Failed, 5=Retried)
- `Enter` → set selected_task to current, switch focus to Detail

**Detail panel keys:**
- `y` → copy selected task's value JSON to clipboard via OSC 52 escape sequence

**Implementation note:** For task list navigation, call `app.visible_tasks()` to get the current filtered list, then index into it. Update both `task_list_state.select(Some(i))` and `app.selected_task = task_ids.get(i).copied()`.

### 2. Wire into main.rs

Add `mod event;` to main.rs.

### 3. Verify

Run `cargo build -p barnum_tui`. Must compile. (You may need to add `base64` to Cargo.toml for the OSC 52 clipboard feature — or defer that and use a simpler `print!("\x1b]52;c;{}\x07", ...)` approach.)

### 4. Commit

```bash
git add crates/barnum_tui/src/event.rs crates/barnum_tui/src/main.rs
git commit -m "feat(tui): add event handling — keybindings for all panels"
```

## Output

```json
[{"kind": "MainLoop", "value": {"workspace_root": "<same>"}}]
```
