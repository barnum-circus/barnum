# MainLoop

Wire all components together into the main TUI event loop — the full integration of config parsing, log watching, rendering, and event handling.

## Input

```json
{"workspace_root": "/path/to/barnum-worktree"}
```

## Context

All modules exist:
- `app.rs` — AppState
- `theme.rs` — colors and styles
- `log_watcher.rs` — LogWatcher
- `graph/` — StepGraph, GraphWidget
- `task_list/` — TaskListWidget
- `detail/` — DetailWidget
- `header.rs` — HeaderWidget
- `footer.rs` — FooterWidget
- `event.rs` — poll_event, handle_key

## Instructions

### 1. Rewrite `crates/barnum_tui/src/main.rs`

Replace the placeholder with the full TUI application. Read the existing main.rs first, then rewrite it.

**Structure:**

```rust
// Module declarations
mod app;
mod detail;
mod event;
mod footer;
mod graph;
mod header;
mod log_watcher;
mod task_list;
mod theme;

// CLI args (already exist from SetupCrate)

fn main() -> anyhow::Result<()> {
    // 1. Parse CLI args
    // 2. Load config: read file, json5::from_str, validate()
    // 3. Build StepGraph from config
    // 4. Create AppState
    // 5. Create LogWatcher (replay mode from CLI flag)
    // 6. Set up terminal: enable_raw_mode, EnterAlternateScreen, Terminal::new
    // 7. Run event loop (in a separate function for clean cleanup)
    // 8. Restore terminal: disable_raw_mode, LeaveAlternateScreen, show_cursor
}
```

**Event loop (`run_loop`):**

```rust
fn run_loop(terminal, app, step_graph, watcher) -> anyhow::Result<()> {
    let tick_rate = Duration::from_millis(100);
    loop {
        // 1. Poll log watcher for new events
        for event in watcher.poll() {
            match event {
                LogEvent::Entry(StateLogEntry::Config(_)) => {} // ignore, graph from file
                LogEvent::Entry(StateLogEntry::TaskSubmitted(sub)) => {
                    app.apply_submitted(sub.task_id, sub.step, sub.value.0, sub.parent_id, sub.origin);
                }
                LogEvent::Entry(StateLogEntry::TaskCompleted(comp)) => {
                    app.apply_completed(comp.task_id, comp.outcome);
                }
                LogEvent::Error(e) => {} // log or ignore
            }
        }

        // 2. Render UI
        terminal.draw(|frame| {
            // Layout: header(1) + body(min 10) + footer(1)
            // Body: [graph+tasklist (min 8)] + [detail (8)]
            // graph+tasklist: 35% | 65%

            // Render header, graph, task list, detail, footer
        })?;

        // 3. Handle input
        if let Some(Event::Key(key)) = event::poll_event(tick_rate) {
            if matches!(event::handle_key(key, app), event::EventResult::Quit) {
                break;
            }
        }
    }
    Ok(())
}
```

**Important:** `StepInputValue` wraps `serde_json::Value`. Access the inner value with `.0` when calling `app.apply_submitted`.

**Layout constants:**
- Header: `Constraint::Length(1)`
- Body: `Constraint::Min(10)`
- Footer: `Constraint::Length(1)`
- Within body: main panels `Constraint::Min(8)` + detail `Constraint::Length(8)`
- Within main: graph `Constraint::Percentage(35)` + task list `Constraint::Percentage(65)`

### 2. Verify

Run `cargo build -p barnum_tui`. Must compile with zero errors. Fix any type mismatches — common issues:
- `StepInputValue` needs `.0` to get inner `serde_json::Value`
- `ListState` clone for stateful table rendering
- Import paths for all widget types

### 3. Commit

```bash
git add crates/barnum_tui/src/main.rs
git commit -m "feat(tui): integrate main loop — full TUI with all panels"
```

## Output

```json
[{"kind": "CLIShim", "value": {"workspace_root": "<same>"}}]
```
