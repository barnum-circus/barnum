# Daemon Event Loop Refactor: Sync Polling → Async Select

## Goal

Replace the current poll-based event loop with a `tokio::select!`-based async loop. This eliminates CPU-wasting polling and periodic scans.

## Current Architecture

### Event Loop Location
`crates/agent_pool/src/daemon/wiring.rs:440-556` - `io_loop()`

### Current Flow (Polling)

```rust
// wiring.rs:455-456
let poll_timeout = Duration::from_millis(100);
let scan_interval = Duration::from_millis(500);

loop {
    // wiring.rs:467-490 - Non-blocking socket accept
    if let Some((raw, stream)) = accept_socket_task(listener)? { ... }

    // wiring.rs:493-529 - Block with 100ms timeout for FS events
    match fs_events.recv_timeout(poll_timeout) { ... }

    // wiring.rs:532-547 - Drain effects (non-blocking)
    while let Ok(effect) = effects_rx.try_recv() { ... }

    // wiring.rs:550-554 - Periodic scans every 500ms
    if last_scan.elapsed() >= scan_interval {
        scan_agents(...)?;
        scan_pending(...)?;
    }
}
```

### Problems

1. **CPU waste**: Polling at 100ms intervals even when idle
2. **Periodic scans**: `scan_agents` and `scan_pending` run every 500ms as a safety net for missed FS events
3. **Three separate event sources**: Socket, FS events, and effects channel are checked sequentially, not concurrently

### Current Code Structure

| File | Purpose |
|------|---------|
| `wiring.rs:440` | `io_loop()` - main event loop |
| `wiring.rs:380` | `run_event_loop_with_shutdown()` - core event loop (processes events → effects) |
| `wiring.rs:320` | `run_daemon()` - sets up watcher, spawns event loop thread |
| `io.rs:380-430` | `execute_effect()` - handles effects (writes files, starts timers) |
| `core.rs:350` | `step()` - pure state machine |

The core/io split is already clean. The refactor is purely in the I/O layer.

---

## Target Architecture

Use `tokio::select!` to wait on multiple async sources simultaneously. No polling.

```rust
loop {
    tokio::select! {
        // Graceful shutdown
        _ = shutdown.cancelled() => break,

        // Socket-based task submission
        result = listener.accept() => { ... }

        // Filesystem events (agent responses, registrations, file submissions)
        Some(event) = fs_rx.recv() => { ... }

        // Effects from core (task assignments, completions)
        Some(effect) = effects_rx.recv() => { ... }

        // Periodic heartbeat checks (only timer remaining)
        _ = heartbeat_interval.tick() => { ... }
    }
}
```

---

## Migration Tasks

### Task 1: Add tokio dependencies

**File:** `crates/agent_pool/Cargo.toml`

```toml
# Add:
tokio = { version = "1", features = ["net", "sync", "time", "rt-multi-thread", "macros"] }
tokio-util = "0.7"  # For CancellationToken
```

### Task 2: Create async channel bridge for notify

**File:** `crates/agent_pool/src/daemon/wiring.rs`

The `notify` crate uses `std::sync::mpsc`. We need to bridge to `tokio::sync::mpsc`.

**Current code (wiring.rs:270-290):**
```rust
let (fs_tx, fs_events) = mpsc::channel();  // std::sync::mpsc
let mut watcher = notify::recommended_watcher(move |event| {
    if let Ok(event) = event {
        let _ = fs_tx.send(event);
    }
})?;
```

**New code:**
```rust
let (fs_tx, mut fs_rx) = tokio::sync::mpsc::channel(256);

// Bridge thread: forwards std channel to tokio channel
let (notify_tx, notify_rx) = std::sync::mpsc::channel();
std::thread::spawn(move || {
    while let Ok(event) = notify_rx.recv() {
        if fs_tx.blocking_send(event).is_err() {
            break;
        }
    }
});

let mut watcher = notify::recommended_watcher(move |event| {
    if let Ok(event) = event {
        let _ = notify_tx.send(event);
    }
})?;
```

### Task 3: Convert io_loop to async

**File:** `crates/agent_pool/src/daemon/wiring.rs:440-556`

**Current signature:**
```rust
fn io_loop(
    listener: &Listener,
    fs_events: &mpsc::Receiver<notify::Event>,
    events_tx: &mpsc::Sender<Event>,
    effects_rx: &mpsc::Receiver<Effect>,
    // ... other params
) -> io::Result<()>
```

**New signature:**
```rust
async fn io_loop(
    listener: tokio::net::UnixListener,
    mut fs_rx: tokio::sync::mpsc::Receiver<notify::Event>,
    events_tx: tokio::sync::mpsc::Sender<Event>,
    mut effects_rx: tokio::sync::mpsc::Receiver<Effect>,
    shutdown: tokio_util::sync::CancellationToken,
    // ... other params
) -> io::Result<()>
```

**New loop body:**
```rust
loop {
    tokio::select! {
        biased;  // Check shutdown first

        _ = shutdown.cancelled() => {
            info!("shutdown requested");
            return Ok(());
        }

        result = listener.accept() => {
            let (stream, _) = result?;
            handle_socket_submission(stream, &events_tx, /* ... */).await?;
        }

        Some(event) = fs_rx.recv() => {
            handle_fs_event(&event, &events_tx, /* ... */);
        }

        Some(effect) = effects_rx.recv() => {
            execute_effect(effect, /* ... */)?;
        }
    }
}
```

### Task 4: Convert event loop thread to tokio task

**File:** `crates/agent_pool/src/daemon/wiring.rs:344-346`

**Current code:**
```rust
let event_loop_handle = thread::spawn(move || {
    run_event_loop_with_shutdown(events_rx, effects_tx, event_loop_signals)
});
```

**Options:**
1. Run core event loop in `tokio::task::spawn_blocking` (keeps it sync)
2. Convert core event loop to async (more work, less benefit since it's pure computation)

Recommend option 1 - core is pure computation, doesn't benefit from async.

### Task 5: Update run_daemon entry point

**File:** `crates/agent_pool/src/daemon/wiring.rs:250`

**Current:**
```rust
pub fn run_daemon(root: &Path, config: DaemonConfig) -> io::Result<Infallible> {
    // ... sync setup ...
    io_loop(...)?;
}
```

**New:**
```rust
pub fn run_daemon(root: &Path, config: DaemonConfig) -> io::Result<Infallible> {
    let rt = tokio::runtime::Runtime::new()?;
    rt.block_on(async {
        // ... async setup ...
        io_loop(...).await
    })
}
```

### Task 6: Remove periodic scans

**File:** `crates/agent_pool/src/daemon/wiring.rs:550-554`

**Remove:**
```rust
// Periodic scans for reliability
if last_scan.elapsed() >= scan_interval {
    scan_agents(...)?;
    scan_pending(...)?;
    last_scan = Instant::now();
}
```

Keep `scan_agents` and `scan_pending` functions for:
- Initial startup scan (existing at wiring.rs:349)
- Manual recovery if needed

### Task 7: Update DaemonHandle for async shutdown

**File:** `crates/agent_pool/src/daemon/wiring.rs:77-120`

Currently uses `DaemonSignals` with `AtomicBool`. Replace with `CancellationToken`:

```rust
pub struct DaemonHandle {
    shutdown: CancellationToken,
    handle: Option<thread::JoinHandle<...>>,
}

impl DaemonHandle {
    pub fn shutdown(self) -> io::Result<()> {
        self.shutdown.cancel();
        // ...
    }
}
```

### Task 8: Update tests

**File:** `crates/agent_pool/src/daemon/wiring.rs` (tests module at bottom)

Tests that spawn the daemon need `#[tokio::test]` attribute:

```rust
#[tokio::test]
async fn event_loop_processes_events_and_emits_effects() {
    // ...
}
```

---

## What Stays the Same

- **Core state machine** (`core.rs`) - pure, no async needed
- **Effect execution** (`io.rs:execute_effect`) - sync file I/O is fine
- **Path categorization** (`path_category.rs`) - pure functions
- **Public API** (`lib.rs` exports) - `run()`, `spawn()`, `submit()` signatures unchanged

---

## Open Questions

1. **Keep interprocess crate?** Currently using `interprocess::local_socket` for cross-platform sockets. With tokio, we could use `tokio::net::UnixListener` directly. Windows would need separate handling.

2. **Async file I/O?** Currently using sync `std::fs`. Could use `tokio::fs` but probably overkill - file ops are fast and we're not I/O bound.

3. **Remove scans entirely?** Or keep as a low-frequency fallback (every 30s instead of 500ms)?

---

## Estimated Scope

- ~300 lines of code changes in `wiring.rs`
- ~50 lines in `io.rs` for async channel types
- ~20 lines in `Cargo.toml`
- Test updates

The core/io split is already clean, so this is primarily a wiring change.
