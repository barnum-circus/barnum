# Unified Watcher Verification

## Status: PENDING

Unify all file watcher usage to follow a consistent pattern with canary-based verification.

## Goals

1. **One watcher per use case** - no creating multiple watchers for a single operation
2. **Every watcher verified with canary** - confirm watcher works before relying on it
3. **Short-circuit on target file** - if we see the file we're waiting for, watcher is implicitly verified
4. **Return the watcher** - caller can continue using it for subsequent waits

## Current Problems

### Problem 1: Double watcher in submit_file

Currently `submit_file()` creates two watchers:

```rust
// First watcher: wait_for_pool_ready()
pub fn submit_file(...) {
    wait_for_pool_ready(root, timeout)?;  // Creates watcher #1, waits for status file

    // ... later in the new notify-based implementation ...
    let watcher = ...;  // Creates watcher #2, waits for response
}
```

### Problem 2: Inconsistent canary patterns

Different places do canary verification differently:
- `wait_for_pool_ready` - has canary verification
- `daemon/wiring.rs` - has canary verification (complex multi-directory)
- `agent.rs` - no canary verification currently

### Problem 3: Can't reuse watchers

Each function creates and discards its own watcher. No way to reuse a verified watcher for multiple waits.

---

## Design

### Key Insight

Verification is **lazy**. We don't need to block waiting for the canary before doing useful work. We just need to know the watcher works before we *rely* on it to see events.

The flow:
1. Create watcher, write canary (non-blocking)
2. Do other work (check if files exist, write requests, etc.)
3. When we need to wait for an event, canary verification happens alongside

### Core Abstraction

```rust
/// A file watcher with lazy canary verification.
pub struct VerifiedWatcher {
    _watcher: RecommendedWatcher,
    state: WatcherState,
}

enum WatcherState {
    Connected {
        rx: mpsc::Receiver<PathBuf>,
        canary: Option<CanaryGuard>,  // Some = unverified, None = verified
    },
    Disconnected,
}

impl VerifiedWatcher {
    /// Create a watcher and start canary verification (non-blocking).
    pub fn new(watch_dir: &Path, canary_path: PathBuf) -> io::Result<Self>;

    /// Block until watcher is verified. Panics if disconnected.
    pub fn ensure_verified(&mut self, timeout: Option<Duration>) -> io::Result<()>;

    /// Wait for a specific file. Panics if disconnected.
    pub fn wait_for(&mut self, target: &Path, timeout: Option<Duration>) -> io::Result<()>;

    /// Return the raw receiver. Panics if disconnected or unverified.
    pub fn into_receiver(self) -> mpsc::Receiver<PathBuf>;
}
```

### State Design

The primary distinction is **Connected vs Disconnected**. Within Connected:
- `canary: Some(guard)` = unverified, canary file exists on disk
- `canary: None` = verified, canary was cleaned up

The `CanaryGuard` struct owns the canary path and cleans up the file when dropped.
State transitions from unverified to verified simply set `canary = None`, which drops
the guard and deletes the file.

### Canary Retry Logic

When not yet verified, `wait_for()` and `ensure_verified()`:
1. Canary file already written by constructor (content `"0"`)
2. Use `recv_timeout(100ms)` to poll for events
3. If ANY event seen → set `canary = None` (drops guard, deletes file, marks verified)
4. If target seen → return success
5. If timeout without events → call `canary.retry()` to rewrite with incrementing number
6. Repeat until done or timeout exceeded

### Key Assumption

**Once we observe any filesystem event, the watcher is fully operational.**

Filesystem watchers (FSEvents on macOS, inotify on Linux) don't "partially work". The only failure mode is during initial setup - there's a brief window after `watch()` returns where events might not be delivered yet. Once we receive ANY event, we can trust that:
- The watcher is fully registered with the kernel
- All subsequent filesystem operations in the watched directory will generate events
- We won't miss events due to setup races

This is why seeing the canary event (or any other event) is sufficient proof that the watcher works for all future operations.

---

## Use Cases

### Overview

| Use Case | Flow | Method Used | Description |
|----------|------|-------------|-------------|
| `submit_file` | **Client → Daemon** | `new()` + `wait_for()` | Lazy verification during wait |
| Daemon startup | **Daemon init** | `new()` + `ensure_verified()` | Explicit verification before status |
| Agent task wait | **Agent → Daemon** | `new()` + `wait_for()` | Lazy verification during wait |

Note: `wait_for_pool_ready` is subsumed by the submission flow - it's just `wait_for(&status_path)`.

---

### 1. File-Based Submission (`submit_file`)

**Flow: Client → Daemon** (submitter waiting for task completion)

**Current flow (polling + two watchers):**
```rust
pub fn submit_file_with_timeout(...) -> io::Result<Response> {
    wait_for_pool_ready(root, timeout)?;  // Creates watcher #1
    // ... write request ...
    // ... poll for response with thread::sleep ...
}
```

**New flow (lazy verification, one watcher):**
```rust
pub fn submit_file_with_timeout(
    root: impl AsRef<Path>,
    payload: &Payload,
    timeout: Option<Duration>,
) -> io::Result<Response> {
    let root = fs::canonicalize(root.as_ref())?;
    let submissions_dir = root.join(SUBMISSIONS_DIR);
    let submission_id = Uuid::new_v4().to_string();

    let request_path = submissions_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = submissions_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));
    let canary_path = submissions_dir.join(format!("{submission_id}.canary"));
    let status_path = root.join(STATUS_FILE);

    // Create watcher, start canary verification (non-blocking)
    let mut watcher = VerifiedWatcher::new(&submissions_dir, canary_path)?;

    // Wait for pool ready (returns immediately if exists)
    watcher.wait_for(&status_path, Some(Duration::from_secs(10)))?;

    // Write request
    atomic_write_str(&root, &request_path, &serde_json::to_string(payload)?)?;

    // Wait for response
    watcher.wait_for(&response_path, timeout)?;

    read_and_cleanup_response(&request_path, &response_path)
}
```

**Key points:**
- Constructor is non-blocking - starts verification but doesn't wait
- `wait_for()` checks existence first, returns immediately if file exists
- Canary verification happens lazily during `wait_for()` calls

### 2. Daemon Startup (`daemon/wiring.rs`)

**Flow: Daemon init** (verifying filesystem watchers work before writing status)

**Current implementation:**
```rust
// In sync_and_setup():
// - Creates watcher on pool root
// - Creates agents/, submissions/, canary
// - Waits to see events for all created items
// - Complex logic to track which events we've seen
```

**New implementation:**

The daemon needs to verify the watcher works BEFORE writing the status file (which signals to clients that it's ready).

```rust
fn setup_daemon(root: &Path) -> io::Result<VerifiedWatcher> {
    let agents_dir = root.join(AGENTS_DIR);
    let submissions_dir = root.join(SUBMISSIONS_DIR);
    let canary_path = root.join("daemon.canary");
    let status_path = root.join(STATUS_FILE);

    // Create directories
    fs::create_dir_all(&agents_dir)?;
    fs::create_dir_all(&submissions_dir)?;

    // Create watcher, explicitly verify (no target to wait for)
    let mut watcher = VerifiedWatcher::new(root, canary_path)?;
    watcher.ensure_verified(Some(Duration::from_secs(5)))?;

    // NOW we know watcher works - safe to signal ready
    fs::write(&status_path, "ready")?;

    Ok(watcher)
}
```

**Key points:**
- `ensure_verified()` explicitly blocks until canary seen
- Status file written AFTER verification (clients can trust watcher works)
- Returns watcher for use in main daemon loop via `into_receiver()`

### 3. Agent Waiting for Task (`agent.rs`)

**Flow: Agent → Daemon** (registered agent waiting for work assignment)

**Current implementation:**
```rust
pub fn wait_for_task(agent_dir: &Path, timeout: Duration) -> io::Result<String> {
    // ... polling loop with thread::sleep ...
}
```

**New implementation:**
```rust
pub fn wait_for_task(agent_dir: &Path, timeout: Option<Duration>) -> io::Result<String> {
    let task_path = agent_dir.join(TASK_FILE);
    let canary_path = agent_dir.join("agent.canary");

    let mut watcher = VerifiedWatcher::new(agent_dir, canary_path)?;
    watcher.wait_for(&task_path, timeout)?;

    fs::read_to_string(&task_path)
}
```

**Key points:**
- `wait_for()` returns immediately if task exists
- Lazy verification if we actually need to wait
- Same simple pattern as all client flows

---

## Implementation Plan

### Step 1: Create `VerifiedWatcher` in `fs_util.rs`

Add to existing `fs_util.rs`:

```rust
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// Guard that cleans up the canary file when dropped.
struct CanaryGuard {
    path: PathBuf,
    writes: u32,
}

impl CanaryGuard {
    fn new(path: PathBuf) -> io::Result<Self> {
        fs::write(&path, "0")?;
        Ok(Self { path, writes: 0 })
    }

    fn retry(&mut self) -> io::Result<()> {
        self.writes += 1;
        fs::write(&self.path, self.writes.to_string())
    }
}

impl Drop for CanaryGuard {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

/// Internal state of the watcher.
enum WatcherState {
    /// Watcher is operational. Has receiver and optional canary guard.
    /// - `canary: Some(_)` = unverified, still waiting for first event
    /// - `canary: None` = verified, canary was cleaned up
    Connected {
        rx: mpsc::Receiver<PathBuf>,
        canary: Option<CanaryGuard>,
    },
    /// Channel disconnected; watcher is broken.
    Disconnected,
}

/// A file watcher with lazy canary verification.
pub struct VerifiedWatcher {
    _watcher: RecommendedWatcher,
    state: WatcherState,
}

impl VerifiedWatcher {
    /// Create a watcher and start canary verification (non-blocking).
    ///
    /// Writes the canary file but returns immediately.
    pub fn new(watch_dir: &Path, canary_path: PathBuf) -> io::Result<Self> {
        let (tx, rx) = mpsc::channel();

        let mut watcher = RecommendedWatcher::new(
            move |res: Result<notify::Event, notify::Error>| {
                if let Ok(event) = res {
                    for path in event.paths {
                        let _ = tx.send(path);
                    }
                }
            },
            Config::default(),
        )
        .map_err(io::Error::other)?;

        watcher
            .watch(watch_dir, RecursiveMode::NonRecursive)
            .map_err(io::Error::other)?;

        let canary = CanaryGuard::new(canary_path)?;

        Ok(Self {
            _watcher: watcher,
            state: WatcherState::Connected {
                rx,
                canary: Some(canary),
            },
        })
    }

    /// Block until watcher is verified.
    ///
    /// Verification succeeds when ANY filesystem event is observed (canary or otherwise).
    /// This relies on the assumption that filesystem watchers don't "partially work" -
    /// once an event is delivered, the watcher is fully operational and will continue
    /// delivering events for subsequent filesystem operations.
    ///
    /// Use when you need verification without waiting for a target file.
    ///
    /// # Panics
    ///
    /// Panics if called when watcher is disconnected.
    pub fn ensure_verified(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        let WatcherState::Connected { rx, canary } = &mut self.state else {
            panic!("ensure_verified called on disconnected watcher");
        };

        // Already verified
        if canary.is_none() {
            return Ok(());
        }

        let start = Instant::now();
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(_) => {
                    // Any filesystem event proves the watcher is working.
                    // Drop the canary guard to clean up the file.
                    *canary = None;
                    return Ok(());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(t) = timeout {
                        if start.elapsed() > t {
                            return Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "watcher verification timed out",
                            ));
                        }
                    }
                    if let Some(c) = canary {
                        c.retry()?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.state = WatcherState::Disconnected;
                    panic!("watcher disconnected unexpectedly");
                }
            }
        }
    }

    /// Wait for a specific file to appear.
    ///
    /// If not yet verified, handles canary verification alongside waiting.
    ///
    /// # Panics
    ///
    /// Panics if called when watcher is disconnected.
    pub fn wait_for(&mut self, target: &Path, timeout: Option<Duration>) -> io::Result<()> {
        // Fast path: file already exists
        if target.exists() {
            return Ok(());
        }

        let WatcherState::Connected { rx, canary } = &mut self.state else {
            panic!("wait_for called on disconnected watcher");
        };

        let start = Instant::now();
        loop {
            // Check timeout
            if let Some(t) = timeout {
                if start.elapsed() > t {
                    if target.exists() {
                        return Ok(());
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("timed out waiting for {}", target.display()),
                    ));
                }
            }

            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(path) => {
                    // Any event proves watcher works
                    if canary.is_some() {
                        *canary = None;
                    }

                    if path == target {
                        return Ok(());
                    }
                    if target.exists() {
                        return Ok(());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if target.exists() {
                        return Ok(());
                    }
                    if let Some(c) = canary {
                        c.retry()?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    self.state = WatcherState::Disconnected;
                    panic!("watcher disconnected unexpectedly");
                }
            }
        }
    }

    /// Consume the watcher and return the raw event receiver.
    ///
    /// Use this for daemon main loops that need to process arbitrary events.
    ///
    /// # Panics
    ///
    /// Panics if called when watcher is disconnected or unverified.
    pub fn into_receiver(self) -> mpsc::Receiver<PathBuf> {
        match self.state {
            WatcherState::Connected { rx, canary: None } => rx,
            WatcherState::Connected { canary: Some(_), .. } => {
                panic!("into_receiver called on unverified watcher")
            }
            WatcherState::Disconnected => {
                panic!("into_receiver called on disconnected watcher")
            }
        }
    }
}
```

### Step 2: Update `submit_file.rs`

Replace the polling implementation with `VerifiedWatcher`. Remove `wait_for_pool_ready` call - it's now just `wait_for(&status_path)`.

### Step 3: Update daemon watcher verification

Modify `sync_and_setup` to use `VerifiedWatcher::ensure_verified()` before writing status file.

### Step 4: Update agent task waiting

Replace polling in `agent.rs` with `VerifiedWatcher`.

---

## File Changes Summary

| File | Change |
|------|--------|
| `crates/agent_pool/src/fs_util.rs` | Add `VerifiedWatcher`, `CanaryGuard`, `WatcherState` |
| `crates/agent_pool/src/lib.rs` | Export `VerifiedWatcher` |
| `crates/agent_pool/src/client/submit_file.rs` | Use `VerifiedWatcher`, remove `wait_for_pool_ready` call |
| `crates/agent_pool/src/daemon/wiring.rs` | Use `ensure_verified()` before writing status |
| `crates/agent_pool/src/agent.rs` | Use `VerifiedWatcher` for task waiting |

---

## Testing Considerations

1. **Unit tests for `VerifiedWatcher`**
   - Canary verification works
   - Short-circuit on target file works
   - Timeout handling works
   - `wait_for` correctly waits for files

2. **Integration tests unchanged**
   - Tests use CLI, which uses these functions internally
   - Behavior should be identical, just faster

3. **Manual testing**
   - Verify latency improvement (no 100ms polling delay)
   - Verify works on both macOS (FSEvents) and Linux (inotify)
