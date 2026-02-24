# Daemon Event Loop Simplification

## Goal

Simplify the daemon's event loop by:
1. Removing unused pause functionality
2. Replacing explicit shutdown signaling with channel closing
3. Unifying multiple event channels into a single `IoEvent` enum

## Current Architecture

### Event Sources (3 separate channels + wake channel)

```
┌─────────────────┐     ┌──────────────┐
│   FS Watcher    │────▶│  fs_events   │──┐
└─────────────────┘     └──────────────┘  │
                                          │
┌─────────────────┐     ┌──────────────┐  │    ┌──────────┐
│ Socket Acceptor │────▶│  socket_rx   │──┼───▶│ wake_rx  │──▶ io_loop
└─────────────────┘     └──────────────┘  │    └──────────┘
                                          │
┌─────────────────┐     ┌──────────────┐  │
│   Event Loop    │────▶│  effects_rx  │──┘
└─────────────────┘     └──────────────┘
```

Each source sends to its own channel AND sends `()` to `wake_tx`.
The io_loop blocks on `wake_rx`, then drains all three channels non-blocking.

### Shutdown Mechanism

- `DaemonSignals` holds `Arc<AtomicU32>` with states: Playing, Paused, Shutdown
- `DaemonHandle::shutdown()` sets the atomic to Shutdown
- io_loop uses `recv_timeout(1 sec)` to periodically check the atomic flag

### Pause Mechanism

- `DaemonHandle::pause()/resume()` toggle the atomic state
- io_loop checks `is_paused()` before processing socket events
- **Never actually used anywhere in the codebase**

## Target Architecture

### Single Unified Channel

```
┌─────────────────┐
│   FS Watcher    │──┐
└─────────────────┘  │
                     │     ┌──────────────┐
┌─────────────────┐  │     │              │
│ Socket Acceptor │──┼────▶│   io_rx      │──▶ io_loop
└─────────────────┘  │     │              │
                     │     └──────────────┘
┌─────────────────┐  │
│   Event Loop    │──┘
└─────────────────┘
```

```rust
enum IoEvent {
    Fs(notify::Event),
    Socket(String, Stream),
    Effect(Effect),
}
```

### Shutdown via Channel Closing

- `DaemonHandle` holds `Sender<IoEvent>` (or just drops it on shutdown)
- When sender is dropped, `io_rx.recv()` returns `Err(RecvError)`
- io_loop exits on channel close - no polling needed

```rust
loop {
    match io_rx.recv() {
        Ok(IoEvent::Fs(event)) => handle_fs_event(...),
        Ok(IoEvent::Socket(raw, stream)) => handle_socket(...),
        Ok(IoEvent::Effect(effect)) => execute_effect(...),
        Err(RecvError) => break, // Channel closed = shutdown
    }
}
```

## Refactor Steps

### Step 1: Remove Pause

**Files:** `crates/agent_pool/src/daemon/wiring.rs`

Remove:
- `DaemonState` enum (lines 78-98)
- `DaemonSignals::set_paused()`, `is_paused()` methods
- `DaemonHandle::pause()`, `resume()`, `is_paused()` methods
- The `if !signals.is_paused()` check in io_loop (line 534)

Keep (for now):
- `DaemonSignals` with just shutdown functionality
- `trigger_shutdown()`, `is_shutdown_triggered()`

This is a safe, isolated change. Tests should pass after.

### Step 2: Replace Shutdown with Channel Closing

**Files:** `crates/agent_pool/src/daemon/wiring.rs`

Changes:
1. `DaemonHandle` holds `Option<Sender<IoEvent>>` instead of just `DaemonSignals`
2. `shutdown()` drops the sender (implicitly by consuming self)
3. Remove `DaemonSignals` entirely
4. io_loop uses blocking `recv()` instead of `recv_timeout()`
5. io_loop exits on `Err(RecvError)` (channel closed)

**Complication:** Currently the sender is created inside `run_daemon`. Need to create it in `spawn_with_config` and pass it in.

### Step 3: Unify Event Channels

**Files:** `crates/agent_pool/src/daemon/wiring.rs`

1. Define `IoEvent` enum
2. Change `create_fs_watcher()` to take `Sender<IoEvent>` parameter
3. Change `spawn_socket_accept_thread()` to take `Sender<IoEvent>` parameter
4. Change event loop to send `IoEvent::Effect` instead of separate channel
5. Simplify io_loop to just recv from one channel
6. Remove wake channel entirely

## Code Changes Summary

### Before (wiring.rs)

```rust
// Multiple channels
let (wake_tx, wake_rx) = mpsc::channel();
let (fs_tx, fs_events) = mpsc::channel();
let (socket_tx, socket_rx) = mpsc::channel();
let (effects_tx, effects_rx) = mpsc::channel();

// Send pattern (repeated 3 times)
tx.send(event);
wake_tx.send(()); // Also wake

// Receive pattern
loop {
    wake_rx.recv_timeout(1sec)?;
    if signals.is_shutdown_triggered() { break; }
    while let Ok(e) = socket_rx.try_recv() { ... }
    while let Ok(e) = fs_events.try_recv() { ... }
    while let Ok(e) = effects_rx.try_recv() { ... }
}
```

### After

```rust
enum IoEvent {
    Fs(notify::Event),
    Socket(String, Stream),
    Effect(Effect),
}

// Single channel
let (io_tx, io_rx) = mpsc::channel();

// Send pattern (each source)
io_tx.send(IoEvent::Fs(event));

// Receive pattern
loop {
    match io_rx.recv() {
        Ok(IoEvent::Fs(e)) => handle_fs(e),
        Ok(IoEvent::Socket(raw, stream)) => handle_socket(raw, stream),
        Ok(IoEvent::Effect(e)) => execute_effect(e),
        Err(_) => break, // Shutdown
    }
}
```

## Testing

After each step, run:
```bash
cargo test --workspace
```

The pre-commit hook will catch any issues.

## Notes

- Step 1 can be done independently and immediately
- Steps 2 and 3 can be done together or separately
- No external API changes (DaemonHandle still has shutdown())
- The behavioral change: events processed one-at-a-time instead of draining all. This is fine (more fair).
