# Multi-Directory Canary Verification

## Status: PENDING APPROVAL

## Problem

On Linux with inotify, recursive file watching has a race condition. When `notify` watches a directory recursively and a new subdirectory is created, `notify` dynamically adds a watch for that subdirectory. There's a window where files can be written to the subdirectory before the watch is set up, causing events to be missed.

### Evidence

CI test `greeting_casual_and_formal::case_1` fails intermittently:
1. Daemon starts, watches pool root recursively
2. Daemon creates `agents/` directory
3. Agent writes `.ready.json` to `agents/`
4. Daemon never sees the `.ready.json` event
5. Task submission times out waiting for an agent

The daemon's canary file is written to the pool root, which proves the root watch works, but says nothing about whether the `agents/` subdirectory watch is ready.

### Flawed Assumption

Both `VerifiedWatcher` and the daemon's `sync_and_setup` have this comment:

```rust
/// Once we observe any filesystem event, the watcher is fully operational.
/// Filesystem watchers (`FSEvents` on macOS, `inotify` on Linux) don't "partially work".
```

This is **incorrect** for recursive watching on Linux. The root watch may work while subdirectory watches are still being set up.

---

## Implementation Plan

| Phase | Goal | Risk |
|-------|------|------|
| 1 | Refactor `VerifiedWatcher`, daemon uses it | Low - behavior unchanged |
| 2 | Enable multi-directory verification | Medium - new behavior |

---

## Phase 1: Refactor VerifiedWatcher, Daemon Uses It

Phase 1 is broken into independently shippable changes:

| Change | Description | Dependencies |
|--------|-------------|--------------|
| 1a | Update `CanaryGuard` API | None |
| 1b | Update `WatcherState` to use `Vec<CanaryGuard>` | 1a |
| 1c | Restore `into_receiver` method | 1b |
| 1d | Refactor daemon to use `VerifiedWatcher` | 1c |

---

### Change 1a: Update CanaryGuard API

**Goal:** `CanaryGuard` generates its own canary path from a directory.

**Before:**
```rust
struct CanaryGuard {
    path: PathBuf,
    writes: u32,
}

impl CanaryGuard {
    fn new(path: PathBuf) -> io::Result<Self> {
        fs::write(&path, "0")?;
        Ok(Self { path, writes: 0 })
    }
}

// Caller constructs path:
let canary_path = dir.join(format!("{}.canary", Uuid::new_v4()));
let canary = CanaryGuard::new(canary_path)?;
```

**After:**
```rust
struct CanaryGuard {
    path: PathBuf,
    dir: PathBuf,
    writes: u32,
}

impl CanaryGuard {
    fn new(dir: PathBuf) -> io::Result<Self> {
        let path = dir.join(format!("{}.canary", Uuid::new_v4()));
        fs::write(&path, "0")?;
        Ok(Self { path, dir, writes: 0 })
    }

    fn dir(&self) -> &Path {
        &self.dir
    }

    fn retry(&mut self) -> io::Result<()> {
        self.writes += 1;
        fs::write(&self.path, self.writes.to_string())
    }
}

// Caller passes directory:
let canary = CanaryGuard::new(dir.clone())?;
```

**Files changed:**
- `verified_watcher.rs`: Update `CanaryGuard`

---

### Change 1b: Update WatcherState

**Goal:** Use `Vec<CanaryGuard>` instead of `Option<CanaryGuard>`. Clearer semantics: empty = verified.

**Before:**
```rust
enum WatcherState {
    Connected {
        rx: mpsc::Receiver<PathBuf>,
        canary: Option<CanaryGuard>,  // None = verified OR never created (ambiguous)
    },
    Disconnected,
}
```

**After:**
```rust
enum WatcherState {
    Connected {
        rx: mpsc::Receiver<notify::Event>,  // Full event, not just path
        remaining_canaries: Vec<CanaryGuard>,  // Empty = verified (clear)
    },
    Disconnected,
}
```

**Update `VerifiedWatcher::new`:**
```rust
pub fn new(watch_dir: &Path, canary_dir: PathBuf) -> io::Result<Self> {
    let (tx, rx) = mpsc::channel();

    let watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| {
            if let Ok(event) = res {
                let _ = tx.send(event);  // Send full event
            }
        },
        Config::default(),
    )?;
    watcher.watch(watch_dir, RecursiveMode::Recursive)?;

    let canary = CanaryGuard::new(canary_dir)?;

    Ok(Self {
        watcher,  // No longer prefixed with _ since into_receiver uses it
        state: WatcherState::Connected {
            rx,
            remaining_canaries: vec![canary],
        },
    })
}
```

**Update `wait_for`:**
```rust
pub fn wait_for(&mut self, target: &Path, timeout: Option<Duration>) -> io::Result<()> {
    if target.exists() {
        return Ok(());
    }

    let WatcherState::Connected { rx, remaining_canaries } = &mut self.state else {
        return Err(io::Error::new(io::ErrorKind::NotConnected, "watcher disconnected"));
    };

    let start = Instant::now();
    loop {
        if let Some(t) = timeout && start.elapsed() > t {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!("timed out waiting for {}", target.display()),
            ));
        }

        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(event) => {
                // Remove canary for verified directories
                for path in &event.paths {
                    if let Some(parent) = path.parent() {
                        remaining_canaries.retain(|c| c.dir() != parent);
                    }

                    if path == target {
                        return Ok(());
                    }
                }

                if target.exists() {
                    return Ok(());
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if target.exists() {
                    return Ok(());
                }
                // Retry only unverified canaries
                for canary in remaining_canaries.iter_mut() {
                    canary.retry()?;
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher disconnected",
                ));
            }
        }
    }
}
```

**Files changed:**
- `verified_watcher.rs`: Update `WatcherState`, `new`, `wait_for`

---

### Change 1c: Restore `into_receiver` Method

**Goal:** Allow callers to consume the watcher and get the raw receiver for custom event loops.

This method previously existed but was removed as dead code in commit `9f10353`. We restore it with the correct signature that returns both the watcher and receiver (caller must keep watcher alive).

```rust
impl VerifiedWatcher {
    /// Block until verified, then return the watcher and receiver.
    ///
    /// Waits until events have been seen from all canary directories,
    /// then consumes self and returns the underlying watcher and receiver
    /// for use in a custom event loop.
    ///
    /// The caller must keep the returned `RecommendedWatcher` alive -
    /// dropping it stops the filesystem watch.
    ///
    /// # Errors
    ///
    /// Returns an error if verification times out.
    pub fn into_receiver(
        mut self,
        timeout: Duration,
    ) -> io::Result<(RecommendedWatcher, mpsc::Receiver<notify::Event>)> {
        let WatcherState::Connected { rx, remaining_canaries } = &mut self.state else {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "watcher disconnected"));
        };

        let start = Instant::now();
        while !remaining_canaries.is_empty() {
            let remaining = timeout.saturating_sub(start.elapsed());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("timed out verifying {} directories", remaining_canaries.len()),
                ));
            }

            let poll = Duration::from_millis(100).min(remaining);
            match rx.recv_timeout(poll) {
                Ok(event) => {
                    for path in &event.paths {
                        if let Some(parent) = path.parent() {
                            remaining_canaries.retain(|c| c.dir() != parent);
                        }
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    for canary in remaining_canaries.iter_mut() {
                        canary.retry()?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "watcher disconnected",
                    ));
                }
            }
        }

        // Extract watcher and rx
        let rx = match std::mem::replace(&mut self.state, WatcherState::Disconnected) {
            WatcherState::Connected { rx, .. } => rx,
            WatcherState::Disconnected => unreachable!(),
        };

        Ok((self.watcher, rx))
    }
}
```

**Usage:**
```rust
let verified_watcher = VerifiedWatcher::new(&root, &[agents_dir, submissions_dir])?;
let (_watcher, rx) = verified_watcher.into_receiver(Duration::from_secs(5))?;
// _watcher must stay in scope to keep the watch alive
// rx yields notify::Event for each filesystem event
```

**Files changed:**
- `verified_watcher.rs`: Add `into_receiver` method, change `_watcher` to `watcher` (no longer unused)

---

### Change 1d: Refactor Daemon to Use VerifiedWatcher

**Goal:** Replace `create_fs_watcher` + inline `sync_and_setup` canary logic with `VerifiedWatcher`.

**Before (wiring.rs):**
```rust
pub fn run_with_config(root: impl AsRef<Path>, config: DaemonConfig) -> io::Result<Infallible> {
    // ...
    let (io_tx, io_rx) = mpsc::channel();
    let _fs_watcher = create_fs_watcher(&root, io_tx.clone())?;

    let (_lock, listener) = sync_and_setup(
        &root, &lock_path, &socket_path,
        &submissions_dir, &agents_dir, &scratch_dir,
        &io_rx,
    )?;
    // ...
}

fn sync_and_setup(..., io_rx: &mpsc::Receiver<IoEvent>) -> io::Result<(LockGuard, Listener)> {
    let canary_path = root.join("daemon.canary");

    fs::create_dir_all(submissions_dir)?;
    fs::create_dir_all(agents_dir)?;
    fs::create_dir_all(scratch_dir)?;

    fs::write(&canary_path, "0")?;

    // Inline verification loop...
    while start.elapsed() < MAX_DURATION {
        match io_rx.recv_timeout(POLL_TIMEOUT) {
            Ok(IoEvent::Fs(event)) => {
                let _ = fs::remove_file(&canary_path);
                return Ok((lock, listener));
            }
            // ...
        }
    }
}
```

**After:**
```rust
pub fn run_with_config(root: impl AsRef<Path>, config: DaemonConfig) -> io::Result<Infallible> {
    // ...

    // Create directories first
    fs::create_dir_all(&submissions_dir)?;
    fs::create_dir_all(&agents_dir)?;
    fs::create_dir_all(&scratch_dir)?;

    // Use VerifiedWatcher - single directory for now (Phase 1)
    let verified_watcher = VerifiedWatcher::new(&root, root.clone())?;
    let (_watcher, fs_rx) = verified_watcher.into_receiver(Duration::from_secs(5))?;

    // Create unified channel
    let (io_tx, io_rx) = mpsc::channel();

    // Spawn thread to forward FS events to unified channel
    let io_tx_fs = io_tx.clone();
    thread::spawn(move || {
        while let Ok(path) = fs_rx.recv() {
            // Convert PathBuf to notify::Event for IoEvent::Fs
            // (or change channel type - see note below)
        }
    });

    let (_lock, listener) = setup_lock_and_socket(&lock_path, &socket_path)?;
    // ...
}
```

**Issue:** `VerifiedWatcher` currently sends `PathBuf`, but daemon needs `notify::Event` for `IoEvent::Fs`. Two options:

1. Change `VerifiedWatcher` to send full `notify::Event` through its channel
2. Keep `PathBuf` and reconstruct minimal event info

Option 1 is cleaner - update the channel type:

```rust
// In VerifiedWatcher
let (tx, rx) = mpsc::channel::<notify::Event>();

// Watcher callback sends full event
move |res: Result<notify::Event, notify::Error>| {
    if let Ok(event) = res {
        let _ = tx.send(event);
    }
}
```

Then daemon can forward directly:
```rust
while let Ok(event) = fs_rx.recv() {
    if io_tx.send(IoEvent::Fs(event)).is_err() {
        break;
    }
}
```

**Files changed:**
- `verified_watcher.rs`: Change channel to `Receiver<notify::Event>`
- `daemon/wiring.rs`: Use `VerifiedWatcher`, remove `create_fs_watcher`, simplify `sync_and_setup`

---

## Phase 2: Multi-Directory Verification

Single change after Phase 1 is complete.

### Change 2: Enable Multi-Directory Verification

**Goal:** Verify multiple directories. Daemon verifies `agents/` and `submissions/`.

**Update `VerifiedWatcher::new` signature:**
```rust
// Before
pub fn new(watch_dir: &Path, canary_dir: PathBuf) -> io::Result<Self>

// After
pub fn new(watch_dir: &Path, canary_dirs: &[PathBuf]) -> io::Result<Self>
```

**Implementation:**
```rust
pub fn new(watch_dir: &Path, canary_dirs: &[PathBuf]) -> io::Result<Self> {
    let (tx, rx) = mpsc::channel();

    let mut watcher = RecommendedWatcher::new(/* ... */)?;
    watcher.watch(watch_dir, RecursiveMode::Recursive)?;

    let remaining_canaries = canary_dirs
        .iter()
        .map(|dir| CanaryGuard::new(dir.clone()))
        .collect::<io::Result<Vec<_>>>()?;

    Ok(Self {
        watcher,
        state: WatcherState::Connected { rx, remaining_canaries },
    })
}
```

**Update callers:**
```rust
// Existing callers (single directory) - wrap in slice
VerifiedWatcher::new(&dir, &[dir.clone()])

// Daemon - verify leaf directories
VerifiedWatcher::new(&root, &[agents_dir.clone(), submissions_dir.clone()])
```

**Files changed:**
- `verified_watcher.rs`: Update `new` signature
- `worker.rs`, `submit/*.rs`: Update callers to pass slice
- `daemon/wiring.rs`: Pass `[agents_dir, submissions_dir]`

---

## Impossible States Made Unrepresentable

### 1. `Option<CanaryGuard>` → `Vec<CanaryGuard>`

Before: `None` could mean "verified" or "never created" - ambiguous.
After: Empty vec = verified. Clear semantics.

### 2. Remove Panic-Only Code Paths

Before: Set `Disconnected` state then panic.
After: Return `Err` directly. `Disconnected` state may be removable.

### 3. Retry Only Unverified Canaries

State encodes what needs retrying:
```rust
// Only remaining (unverified) canaries get retried
for canary in remaining_canaries.iter_mut() {
    canary.retry()?;
}
```

---

## Testing

After all changes:

1. `cargo test -p agent_pool` - all tests pass
2. `cargo test -p agent_pool --test greeting -- --test-threads=1` (multiple runs)
3. All cases should pass consistently
