# Daemon Event Loop Refactor

## Goal

Replace the current poll-based event loop with a blocking approach that wakes on any event. This eliminates CPU-wasting polling.

## Current Architecture

### Event Loop Location
`crates/agent_pool/src/daemon/wiring.rs` - `io_loop()`

### Current Flow (Polling)

```rust
let poll_timeout = Duration::from_millis(100);

loop {
    // Non-blocking socket accept
    if let Some((raw, stream)) = accept_socket_task(listener)? { ... }

    // Block with 100ms timeout for FS events
    match fs_events.recv_timeout(poll_timeout) { ... }

    // Drain effects (non-blocking)
    while let Ok(effect) = effects_rx.try_recv() { ... }
}
```

### Problems

1. **CPU waste**: Polling at 100ms intervals even when idle
2. **Sequential checking**: Socket, FS events, and effects are checked one at a time

### Current Code Structure

| File | Purpose |
|------|---------|
| `wiring.rs` | `io_loop()` - main event loop |
| `wiring.rs` | `run_event_loop_with_shutdown()` - core event loop (processes events → effects) |
| `wiring.rs` | `run_daemon()` - sets up watcher, spawns event loop thread |
| `io.rs` | `execute_effect()` - handles effects (writes files, starts timers) |
| `core.rs` | `step()` - pure state machine |

The core/io split is already clean. The refactor is purely in the I/O layer.

---

## Phase 1: Wake Channel Pattern

Use a shared "wake" channel that all event sources ping. The main loop blocks on this channel, then drains all sources non-blocking.

### Target Architecture

```rust
// Each event source has its own channel + pings the wake channel
let (wake_tx, wake_rx) = mpsc::channel::<()>();

// FS watcher thread
let wake_tx_fs = wake_tx.clone();
std::thread::spawn(move || {
    while let Ok(event) = notify_rx.recv() {
        let _ = fs_tx.send(event);
        let _ = wake_tx_fs.send(());  // Wake main loop
    }
});

// Socket accept thread
let wake_tx_socket = wake_tx.clone();
std::thread::spawn(move || {
    loop {
        if let Ok((raw, stream)) = accept_connection(&listener) {
            let _ = socket_tx.send((raw, stream));
            let _ = wake_tx_socket.send(());  // Wake main loop
        }
    }
});

// Effects are sent from the event loop thread, which also pings wake
// (Already has wake_tx from being spawned)

// Main loop
loop {
    wake_rx.recv()?;  // Block until any source has something

    // Drain all sources (non-blocking)
    while let Ok(event) = fs_rx.try_recv() {
        handle_fs_event(&event, ...);
    }
    while let Ok((raw, stream)) = socket_rx.try_recv() {
        handle_socket_submission(raw, stream, ...);
    }
    while let Ok(effect) = effects_rx.try_recv() {
        execute_effect(effect, ...)?;
    }
}
```

### Benefits

1. **Zero polling**: Main loop blocks until there's actual work
2. **No new dependencies**: Just `std::sync::mpsc`
3. **Simple mental model**: Each producer sends to its channel AND pings wake
4. **Predictable**: No async runtime surprises

### Implementation Tasks

#### Task 1: Add wake channel and socket accept thread

**File:** `crates/agent_pool/src/daemon/wiring.rs`

1. Create the wake channel in `run_daemon()`
2. Spawn a dedicated thread for socket accepts (currently inline in io_loop)
3. Each thread gets a clone of `wake_tx`

#### Task 2: Update io_loop to block on wake channel

**File:** `crates/agent_pool/src/daemon/wiring.rs`

Replace:
```rust
let poll_timeout = Duration::from_millis(100);
loop {
    // ... poll-based checks ...
    match fs_events.recv_timeout(poll_timeout) { ... }
}
```

With:
```rust
loop {
    wake_rx.recv()?;  // Block until woken
    // Drain all sources...
}
```

#### Task 3: Wire up effects channel to wake

**File:** `crates/agent_pool/src/daemon/wiring.rs`

The event loop thread (which runs core) needs to ping `wake_tx` after sending effects. This requires passing `wake_tx` to `run_event_loop_with_shutdown()`.

---

## Phase 2: Consider Tokio

After Phase 1 is working, evaluate whether tokio adds value.

### What Tokio Would Give Us

1. **Cleaner syntax**: `tokio::select!` vs manual wake+drain
2. **Built-in timeouts**: `tokio::time::timeout()` for operations
3. **Cancellation tokens**: Structured shutdown with `CancellationToken`
4. **Ecosystem**: Easy to add HTTP, timers, or other async I/O later

### What Tokio Costs

1. **~100+ transitive dependencies**: Slower compile, bigger binary
2. **Runtime complexity**: `spawn` vs `spawn_blocking`, blocking in async context
3. **Learning curve**: Async Rust has footguns (holding locks across await, etc.)
4. **Startup time**: Runtime initialization isn't free

### Honest Assessment

**For our current use case, the wake channel pattern is sufficient.** We have:
- 3 event sources (FS, socket, effects)
- No need for timeouts on individual operations
- No HTTP or external async I/O
- Simple shutdown semantics (AtomicBool already works)

**Tokio becomes worth it if we add:**
- HTTP webhooks for task completion notifications
- Remote agent connections over TCP
- Complex timeout/retry logic
- Multiple concurrent operations that need cancellation

**Recommendation:** Implement Phase 1. Re-evaluate tokio if/when we need its features. The core/io split means we can add tokio later without touching the state machine.

---

## What Stays the Same

- **Core state machine** (`core.rs`) - pure, no async needed
- **Effect execution** (`io.rs:execute_effect`) - sync file I/O is fine
- **Path categorization** (`path_category.rs`) - pure functions
- **Public API** (`lib.rs` exports) - `run()`, `spawn()`, `submit()` signatures unchanged

---

## Resolved Questions

1. **Keep interprocess crate?** Yes, for cross-platform socket support.
2. **Async file I/O?** No, sync is fine for our use case.
3. **Remove periodic scans?** Already done - we watch root recursively now.
