# DetailPane

Implement the detail pane — shows full context for the selected task including parent chain, outcome details, and pretty-printed JSON value.

## Input

```json
{"workspace_root": "/path/to/barnum-worktree"}
```

## Context

The `barnum_tui` crate exists with `app.rs` and `theme.rs`.

Read:
- `{workspace_root}/crates/barnum_tui/src/app.rs` — AppState, TaskRecord
- `{workspace_root}/crates/barnum_state/src/types.rs` — TaskOutcome, TaskSuccess, TaskFailed, FailureReason, TaskOrigin

## Instructions

### 1. Create `crates/barnum_tui/src/detail/mod.rs`

```rust
pub mod render;
```

### 2. Create `crates/barnum_tui/src/detail/render.rs`

**DetailWidget** — implements `ratatui::widgets::Widget`:

**Constructor:** `new(app: &AppState, focused: bool)`

**Rendering when a task is selected (from `app.selected_task`):**

Build a `Vec<Line>` containing:

1. **Header line:** "Task t-{id} > {step_name} > {icon} {status_label}" — with cyan task ID, white step name, colored status
2. **Duration:** "Duration: {seconds}s" — live-updating for incomplete tasks
3. **Origin:** "Origin: initial/spawned/retry (replaces t-XX)/finally (for t-XX)"
4. **Parent chain:** "Parent chain: t-07 (Impl) <- t-04 (Analyze) <- t-01 (Start)" — walk up via parent_id
5. **Children:** "Children: t-08 (Test), t-09 (Test)" — if any
6. **Outcome details** (if completed):
   - Success: "Spawned: t-08, t-09" (list of spawned task IDs)
   - Failed: "Failure: Timeout/AgentLost/Invalid response: {msg}" in red, plus "Retried as: t-XX" if applicable
7. **Value JSON:** "Value:" header, then pretty-printed JSON with gray color

**Rendering when no task selected:** "Select a task to view details" in dark gray.

**Layout:** Wrap in a `Block` with "Detail" title, focused/unfocused border. Content rendered as a `Paragraph` with `Wrap { trim: false }`.

### 3. Wire into main.rs

Add `mod detail;` to main.rs.

### 4. Verify

Run `cargo build -p barnum_tui`. Must compile.

### 5. Commit

```bash
git add crates/barnum_tui/src/detail/ crates/barnum_tui/src/main.rs
git commit -m "feat(tui): add detail pane — full task info, parent chain, JSON value"
```

## Output

```json
[{"kind": "EventHandling", "value": {"workspace_root": "<same>"}}]
```
