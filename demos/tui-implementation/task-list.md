# TaskList

Implement the task list panel — a table widget showing tasks filtered by selected step, with sorting and status filtering.

## Input

```json
{"workspace_root": "/path/to/barnum-worktree"}
```

## Context

The `barnum_tui` crate exists with `app.rs` (AppState, TaskRecord, visible_tasks()) and `theme.rs`.

Read:
- `{workspace_root}/crates/barnum_tui/src/app.rs` — AppState, TaskRecord, TaskStatus, SortColumn
- `{workspace_root}/crates/barnum_tui/src/theme.rs` — colors, icons, focused/unfocused border styles

## Instructions

### 1. Create `crates/barnum_tui/src/task_list/mod.rs`

Just re-exports:

```rust
pub mod render;
```

### 2. Create `crates/barnum_tui/src/task_list/render.rs`

**TaskListWidget** — renders a `ratatui::widgets::Table`:

**Constructor:** `new(tasks: &[LogTaskId], app: &AppState, focused: bool)`

**Columns:**
| Column | Width | Content |
|--------|-------|---------|
| ID | 6 | `t-{:02}` formatted |
| Status | 14 | Icon + label (e.g., "● in-flight") |
| Step | 12 | Step name (only shown when no step filter active) |
| Duration | 8 | Formatted elapsed time |
| Value | Fill | Truncated JSON preview (max 40 chars + "...") |

**Duration formatting:** `<60s` → "12s", `<3600s` → "2m05s", else → "1h23m"

**Rendering:**
- Use `ratatui::widgets::Table` with `StatefulWidget` for scrollable selection
- Header row: bold + underlined
- Each row styled with the task's status color
- Highlighted row uses `theme::selected_style()`
- Block border uses focused/unfocused style based on panel focus
- Block title shows "Tasks: {step_name}" or "Tasks: All"

**The `render_with_state` method:** Takes `area`, `buf`, and `&mut TableState` — this is a stateful render (not the standard Widget trait) because ratatui Table needs state for selection tracking.

### 3. Wire into main.rs

Add `mod task_list;` to main.rs.

### 4. Verify

Run `cargo build -p barnum_tui`. Must compile.

### 5. Commit

```bash
git add crates/barnum_tui/src/task_list/ crates/barnum_tui/src/main.rs
git commit -m "feat(tui): add task list panel — table widget with filtering and sorting"
```

## Output

```json
[{"kind": "EventHandling", "value": {"workspace_root": "<same>"}}]
```
