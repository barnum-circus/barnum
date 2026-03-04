# Migrate to crossbeam channels

## Motivation

The codebase currently uses `std::sync::mpsc` channels. This works but has limitations:

1. **No `select!`** - Can't wait on multiple channels simultaneously without spawning forwarder threads
2. **Single consumer** - `Receiver` can't be cloned or shared
3. **No `try_recv` with timeout** - Only `recv()` (blocking) or `recv_timeout()` (polling)

The daemon works around this with forwarder threads (one per event source), but this is heavyweight and complex.

`crossbeam::channel` provides:
- `select!` macro for waiting on multiple channels
- Both `bounded` and `unbounded` channels
- `Sender` and `Receiver` are both `Clone + Send + Sync`
- Better performance in many cases

## Current Usage

### Daemon event loop (wiring.rs)

```rust
use std::sync::mpsc;

enum IoEvent {
    Fs(notify::Event),
    Socket(String, Stream),
    Effect(Effect),
}

let (io_tx, io_rx) = mpsc::channel();

// Forwarder thread for FS events
thread::spawn(move || {
    while let Ok(event) = fs_rx.recv() {
        fs_io_tx.send(IoEvent::Fs(event));
    }
});

// Main loop
loop {
    match io_rx.recv() {
        Ok(IoEvent::Fs(e)) => ...,
        Ok(IoEvent::Socket(..)) => ...,
        Ok(IoEvent::Effect(e)) => ...,
    }
}
```

### VerifiedWatcher (verified_watcher.rs)

```rust
use std::sync::mpsc;

let (tx, rx) = mpsc::channel();

let watcher = RecommendedWatcher::new(move |res| {
    if let Ok(event) = res {
        let _ = tx.send(event);
    }
}, ...)?;

// Later
match rx.recv_timeout(Duration::from_millis(100)) {
    Ok(event) => ...,
    Err(Timeout) => ...,
    Err(Disconnected) => ...,
}
```

## Proposed Changes

### 1. Add crossbeam-channel dependency

```toml
# crates/agent_pool/Cargo.toml
[dependencies]
crossbeam-channel = "0.5"
```

Note: We use `crossbeam-channel` (just channels + select!) rather than the full `crossbeam` crate which includes atomics, epoch-based memory management, and other utilities we don't need.

### 2. Update VerifiedWatcher

```rust
use crossbeam_channel::{self, Receiver, Sender};

struct WatcherState {
    rx: Receiver<notify::Event>,
    remaining_canaries: Vec<CanaryGuard>,
}

impl VerifiedWatcher {
    pub fn new(watch_dir: &Path, canary_dirs: &[PathBuf]) -> io::Result<Self> {
        let (tx, rx) = channel::unbounded();

        let watcher = RecommendedWatcher::new(move |res| {
            if let Ok(event) = res {
                let _ = tx.send(event);
            }
        }, ...)?;

        // ...
    }
}
```

### 3. Update daemon wiring

Replace forwarder threads with `select!`:

```rust
use crossbeam_channel::{self, Receiver, Sender, select};

// Instead of unified IoEvent channel with forwarders,
// use select! on the individual channels directly

let (effect_tx, effect_rx) = channel::unbounded();

loop {
    select! {
        recv(fs_rx) -> event => {
            if let Ok(event) = event {
                handle_fs_event(event);
            }
        }
        recv(socket_rx) -> conn => {
            if let Ok((payload, stream)) = conn {
                handle_socket(payload, stream);
            }
        }
        recv(effect_rx) -> effect => {
            if let Ok(effect) = effect {
                execute_effect(effect);
            }
        }
    }
}
```

This eliminates:
- The `IoEvent` enum (each channel carries its native type)
- The forwarder threads
- The complexity of funneling everything into one channel

### 4. Socket accept loop

Currently the socket accept runs in a thread with polling:

```rust
// Current: polling loop
thread::spawn(move || {
    loop {
        match listener.accept() {
            Ok(stream) => socket_tx.send(...),
            Err(e) if e.kind() == WouldBlock => {
                thread::sleep(Duration::from_millis(10));
            }
            Err(_) => break,
        }
    }
});
```

This can stay as-is (it's I/O-bound, not CPU-bound). The socket accept thread forwards to a channel that the main `select!` listens on.

## Migration Path

1. **Add dependency** - Just add crossbeam to Cargo.toml
2. **Update VerifiedWatcher** - Change mpsc to crossbeam::channel (internal change, no API change)
3. **Update into_receiver** - Return `crossbeam::channel::Receiver` instead of `mpsc::Receiver`
4. **Update daemon wiring** - Replace forwarder pattern with `select!`
5. **Remove IoEvent enum** - No longer needed when using `select!`

## API Changes

### VerifiedWatcher::into_receiver

```rust
// Before
pub fn into_receiver(self, timeout: Duration)
    -> io::Result<(RecommendedWatcher, std::sync::mpsc::Receiver<notify::Event>)>;

// After
pub fn into_receiver(self, timeout: Duration)
    -> io::Result<(RecommendedWatcher, crossbeam::channel::Receiver<notify::Event>)>;
```

This is a breaking change for anyone using `into_receiver` directly, but:
- It's not part of the public API (not exported from lib.rs)
- Only the daemon uses it

## Benefits After Migration

1. **Simpler daemon code** - No forwarder threads, no IoEvent enum
2. **Enables cancellation** - Can add cancel channel to any `select!` trivially
3. **Better semantics** - Each channel carries its native type
4. **Foundation for future** - Any new blocking operation can use `select!`

## Testing

- Existing daemon tests should pass unchanged
- Existing watcher tests should pass unchanged
- Add test for select! behavior (receives from whichever channel has data first)
