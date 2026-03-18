# LogWatcher

Implement the state log file watcher that tails the NDJSON state log and yields parsed events.

## Input

```json
{"workspace_root": "/path/to/barnum-worktree"}
```

## Context

The `barnum_tui` crate exists with `app.rs` (AppState, TaskRecord) and `theme.rs` already created.

Read these files for the types you'll parse:
- `{workspace_root}/crates/barnum_state/src/types.rs` — `StateLogEntry` enum with variants: `Config(StateLogConfig)`, `TaskSubmitted(TaskSubmitted)`, `TaskCompleted(TaskCompleted)`
- `{workspace_root}/crates/barnum_state/src/lib.rs` — existing read/write functions for reference

The state log is NDJSON (newline-delimited JSON). Each line is a `StateLogEntry` serialized with `#[serde(tag = "kind")]`. Example:

```
{"kind":"Config","config":{...}}
{"kind":"TaskSubmitted","task_id":0,"step":"Analyze","value":{...},"parent_id":null,"origin":{"kind":"Initial"}}
{"kind":"TaskCompleted","task_id":0,"outcome":{"kind":"Success","value":{"spawned_task_ids":[1],"finally_value":{...}}}}
```

## Instructions

### 1. Create `crates/barnum_tui/src/log_watcher.rs`

**Design:** The LogWatcher uses `notify` to watch for file modifications and reads new lines from the state log on each poll. The watcher signals "file changed" via a channel; actual reading happens on the main thread (avoiding the Fn vs FnMut issue with notify closures).

**Public API:**

```rust
pub enum LogEvent {
    Entry(StateLogEntry),
    Error(String),
}

pub struct LogWatcher {
    reader: BufReader<File>,
    notify_rx: mpsc::Receiver<()>,
    _watcher: RecommendedWatcher,
}

impl LogWatcher {
    /// Create a new watcher.
    /// replay=true: read from beginning. replay=false: seek to end, tail only.
    pub fn new(path: &Path, replay: bool) -> anyhow::Result<Self>;

    /// Drain all pending events (non-blocking). Call every tick (~100ms).
    pub fn poll(&mut self) -> Vec<LogEvent>;
}
```

**Implementation details:**
- `new()`: Open file, optionally seek to end, create a `notify::recommended_watcher` that sends `()` on modify/create events, watch the parent directory (more reliable than watching the file directly)
- `poll()`: Drain notify signals (just to clear the channel), then read all available lines from the BufReader, parse each non-empty line as `StateLogEntry` via `serde_json::from_str`, return parsed events
- Always read lines even without notify signals (the initial replay read happens on first poll)

### 2. Wire into main.rs

Add `mod log_watcher;` to main.rs.

### 3. Verify

Run `cargo build -p barnum_tui`. Must compile.

### 4. Commit

```bash
git add crates/barnum_tui/src/log_watcher.rs crates/barnum_tui/src/main.rs
git commit -m "feat(tui): add LogWatcher — tails NDJSON state log via notify"
```

## Output

```json
[{"kind": "EventHandling", "value": {"workspace_root": "<same>"}}]
```
