# Notify-Based Agent Waiting

## Goal

Replace sleep-based polling with `notify`-based file watching for agents waiting for tasks. Test agents should use the same code path as real agents (via CLI or library).

## Current Architecture

### The Agent Protocol (File-Based)

Agents communicate with the daemon through files in `<pool>/agents/<agent_name>/`:
- `task.json` - Written by daemon when assigning work
- `response.json` - Written by agent when work is complete

**State Machine:**

| task.json | response.json | Meaning |
|-----------|---------------|---------|
| absent | absent | Idle - waiting for task |
| present | absent | Task pending - agent should process |
| present | present | Agent done - daemon should cleanup |
| absent | present | Cleanup in progress - transitionary, do nothing |

The daemon deletes task.json first, then response.json. This means `(absent, present)` is a valid transitionary state that agents will briefly observe during cleanup. Agents should simply wait - they only act when `task.exists() && !response.exists()`.

### Existing Daemon Abstractions

The daemon already has file-based abstractions in `crates/agent_pool/src/daemon/io.rs`:

**`Transport` enum (lines 60-65):**
```rust
pub(super) enum Transport {
    Directory(PathBuf),
    Socket(Stream),
}
```

**`Transport` methods (lines 77-120):**
```rust
impl Transport {
    /// Read content from a file in this transport.
    pub fn read(&self, filename: &str) -> io::Result<String>;

    /// Write content to a file atomically (temp + rename).
    pub fn write(&self, filename: &str, content: &str) -> io::Result<()>;

    /// Get the path for directory-based transports.
    pub fn path(&self) -> Option<&Path>;
}
```

**`TransportMap` (lines 199-312):**
Maps IDs to transports, handles registration/lookup by path.

**Notify-based watching (wiring.rs lines 996-1021):**
```rust
fn create_fs_watcher(
    root: &Path,
    wake_tx: mpsc::Sender<()>,
) -> io::Result<(RecommendedWatcher, mpsc::Receiver<notify::Event>)> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);
                let _ = wake_tx.send(()); // Wake main loop
            }
        },
        config,
    ).map_err(io::Error::other)?;
    // ...
}
```

### Task Envelope Format

The daemon writes envelopes to `task.json` (io.rs lines 387-410):

**Regular tasks:** Pass through content directly from submission
```json
{"kind": "Task", "task": {"instructions": "...", "data": {...}}}
```

**Heartbeats (line 397-406):**
```json
{"kind": "Heartbeat", "task": {"instructions": "...", "data": null}}
```

**Kicked (lines 484-487):**
```json
{"kind": "Kicked", "reason": "Timeout"}
```

### Current Polling Implementations

**CLI (`crates/agent_pool_cli/src/main.rs`):**
```rust
// Lines 195-239
fn wait_for_task(...) -> Result<String, String> {
    loop {
        if task_file.exists() && !response_file.exists() {
            return Ok(...);
        }
        thread::sleep(Duration::from_millis(100));  // POLLING!
    }
}
```

**Test agents (`crates/agent_pool/tests/common/mod.rs`):**
```rust
while running_clone.load(Ordering::SeqCst) {
    if task_file.exists() && !response_file.exists() {
        // Process task...
    }
    thread::sleep(Duration::from_millis(10));  // POLLING!
}
```

## Key Insight: Symmetric Operations

The daemon and agent are doing **symmetric operations**:

| Operation | Daemon | Agent |
|-----------|--------|-------|
| Write | task.json | response.json |
| Watch for | response.json | task.json |
| Read | response.json | task.json |
| Clean up | Both files | - |

Both sides need:
1. Atomic file writes (temp + rename)
2. Notify-based watching
3. State machine logic (when to act based on file existence)

**Question: Can we share the same primitive?**

The `Transport` abstraction in io.rs provides:
- `read(filename)` / `write(filename, content)` - generic file ops
- Atomic writes via temp file
- Path tracking

What's missing for agent use:
- Notify-based watching (currently separate in wiring.rs)
- The "wait for condition" logic

## Design Options

### Option A: Separate Agent Abstraction

Create `AgentTransport` in `agent/` module that:
- Has its own notify watcher
- Implements `wait_for_task()` and `write_response()`
- Duplicates some logic from daemon's Transport

**Pro:** Clean separation, agent code doesn't depend on daemon internals
**Con:** Duplicates the atomic write logic, notify setup

### Option B: Shared Transport with Role-Specific Methods

Move `Transport` to a shared location, add:
```rust
impl Transport {
    // Existing
    fn read(&self, filename: &str) -> io::Result<String>;
    fn write(&self, filename: &str, content: &str) -> io::Result<()>;

    // New: wait for a file condition
    fn wait_for<F>(&self, condition: F) -> io::Result<()>
    where F: Fn(&Path) -> bool;
}
```

Then agent and daemon both use `Transport` with different conditions.

**Pro:** Single source of truth for file ops
**Con:** `Transport` is currently `pub(super)` in daemon module; moving it changes visibility

### Option C: Extract Core Primitives

Extract the core operations into a shared module:
```rust
// crates/agent_pool/src/file_protocol.rs

/// Atomic file write (temp + rename)
pub fn atomic_write(path: &Path, content: &str) -> io::Result<()>;

/// Create a notify watcher for a directory
pub fn create_watcher(dir: &Path) -> io::Result<(Watcher, Receiver<Event>)>;

/// Wait for a condition on files in a directory
pub fn wait_for<F>(rx: &Receiver<Event>, condition: F) -> io::Result<()>
where F: Fn() -> bool;
```

Both daemon and agent use these primitives but compose them differently.

**Pro:** Maximum reuse, clear primitives
**Con:** More refactoring of daemon code

## Recommendation

**Option C** seems most aligned with the "right primitives" philosophy. The daemon already has these operations scattered across io.rs and wiring.rs - we'd be consolidating them.

However, this is a larger refactor. For a quicker path:

1. **First:** Extract just `atomic_write` and the notify watcher setup as shared functions
2. **Then:** Use those in a new `AgentTransport` that agents call
3. **Later:** Refactor daemon to use the same primitives

## Open Questions

1. Should `Transport` remain daemon-only, or become a shared abstraction?
2. The socket transport in `Transport::Socket` - does the agent side ever use sockets, or only files?
3. Should we add a trait `FileProtocol` that both sides implement?

## Next Steps

Before implementing, we should:
1. Decide on Option A, B, or C
2. If B or C, plan the daemon refactoring needed
3. Identify what can be done incrementally vs. all-at-once
