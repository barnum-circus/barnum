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
    rx: mpsc::Receiver<PathBuf>,
    _watcher: RecommendedWatcher,
    canary_path: PathBuf,
    verified: bool,
}

impl VerifiedWatcher {
    /// Create a watcher and start canary verification (non-blocking).
    ///
    /// Writes the canary file but returns immediately. Verification
    /// completes during subsequent `wait_for()` calls, or explicitly
    /// via `ensure_verified()`.
    pub fn new(watch_dir: &Path, canary_path: PathBuf) -> io::Result<Self>;

    /// Block until watcher is verified (canary event seen).
    ///
    /// Use this when you need verification without waiting for a target file.
    /// Example: daemon startup before writing status file.
    pub fn ensure_verified(&mut self, timeout: Option<Duration>) -> io::Result<()>;

    /// Wait for a specific file to appear.
    ///
    /// If not yet verified, uses `recv_timeout` to catch both canary and
    /// target events, retrying canary writes periodically. Once verified,
    /// just waits for the target.
    ///
    /// If the target already exists, returns immediately (no verification needed).
    pub fn wait_for(&mut self, target: &Path, timeout: Option<Duration>) -> io::Result<()>;

    /// Consume the watcher and return the raw event receiver.
    ///
    /// Use this when you need custom event processing (e.g., daemon main loop).
    /// Should only be called after verification (either explicit or implicit).
    pub fn into_receiver(self) -> mpsc::Receiver<PathBuf>;
}
```

### Why One Struct?

Considered a typestate pattern (`MaybeVerifiedWatcher` → `VerifiedWatcher`), but:
- `wait_for()` doesn't cleanly transition states (file might already exist → no verification needed)
- The only API that truly requires verification is `into_receiver()`
- Complexity not worth the type safety gain

Instead, `verified` is tracked internally, and `into_receiver()` should only be called after verification (documented, not enforced by types).

### Canary Retry Logic

When not yet verified, `wait_for()` and `ensure_verified()`:
1. Write canary with content `"sync"`
2. Use `recv_timeout(100ms)` to poll for events
3. If canary seen → mark verified, delete canary
4. If target seen → return success (implicitly verified - we saw an event!)
5. If timeout → rewrite canary with timestamp (triggers new event)
6. Repeat until verified + target seen, or timeout exceeded

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

/// A file watcher with lazy canary verification.
pub struct VerifiedWatcher {
    rx: mpsc::Receiver<PathBuf>,
    _watcher: RecommendedWatcher,
    canary_path: PathBuf,
    verified: bool,
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

        // Write canary to start verification (non-blocking)
        fs::write(&canary_path, "sync")?;

        Ok(Self {
            rx,
            _watcher: watcher,
            canary_path,
            verified: false,
        })
    }

    /// Block until watcher is verified (canary event seen).
    ///
    /// Use when you need verification without waiting for a target file.
    pub fn ensure_verified(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        if self.verified {
            return Ok(());
        }

        let start = Instant::now();
        loop {
            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(path) if path == self.canary_path => {
                    self.verified = true;
                    let _ = fs::remove_file(&self.canary_path);
                    return Ok(());
                }
                Ok(_) => {
                    // Any event proves watcher works
                    self.verified = true;
                    let _ = fs::remove_file(&self.canary_path);
                    return Ok(());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(t) = timeout {
                        if start.elapsed() > t {
                            let _ = fs::remove_file(&self.canary_path);
                            return Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "watcher verification timed out",
                            ));
                        }
                    }
                    // Retry canary write
                    fs::write(&self.canary_path, start.elapsed().as_millis().to_string())?;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = fs::remove_file(&self.canary_path);
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "watcher disconnected",
                    ));
                }
            }
        }
    }

    /// Wait for a specific file to appear.
    ///
    /// If not yet verified, handles canary verification alongside waiting.
    pub fn wait_for(&mut self, target: &Path, timeout: Option<Duration>) -> io::Result<()> {
        // Fast path: file already exists
        if target.exists() {
            return Ok(());
        }

        let start = Instant::now();
        loop {
            // Check timeout
            if let Some(t) = timeout {
                if start.elapsed() > t {
                    if target.exists() {
                        return Ok(());
                    }
                    let _ = fs::remove_file(&self.canary_path);
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("timed out waiting for {}", target.display()),
                    ));
                }
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(path) => {
                    // Any event proves watcher works
                    if !self.verified {
                        self.verified = true;
                        let _ = fs::remove_file(&self.canary_path);
                    }

                    if path == target {
                        return Ok(());
                    }
                    // Check existence in case we missed the event
                    if target.exists() {
                        return Ok(());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Check existence
                    if target.exists() {
                        return Ok(());
                    }
                    // Retry canary if not verified
                    if !self.verified {
                        fs::write(&self.canary_path, start.elapsed().as_millis().to_string())?;
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = fs::remove_file(&self.canary_path);
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "watcher disconnected",
                    ));
                }
            }
        }
    }

    /// Consume the watcher and return the raw event receiver.
    ///
    /// Should only be called after verification.
    pub fn into_receiver(self) -> mpsc::Receiver<PathBuf> {
        // Clean up canary if still exists
        let _ = fs::remove_file(&self.canary_path);
        self.rx
    }
}
```

### Step 2: Update `submit_file.rs`

Replace the polling implementation with `VerifiedWatcher`.

### Step 3: Update `wait_for_pool_ready` in `client/mod.rs`

Simplify to use `VerifiedWatcher`.

### Step 4: Update daemon watcher verification

Modify `sync_and_setup` to use dual-canary verification.

### Step 5: Update agent task waiting

Replace polling in `agent.rs` with `VerifiedWatcher`.

---

## File Changes Summary

| File | Change |
|------|--------|
| `crates/agent_pool/src/fs_util.rs` | Add `VerifiedWatcher` struct |
| `crates/agent_pool/src/lib.rs` | Export `VerifiedWatcher` |
| `crates/agent_pool/src/client/submit_file.rs` | Use `VerifiedWatcher`, single watcher |
| `crates/agent_pool/src/client/mod.rs` | Simplify `wait_for_pool_ready` |
| `crates/agent_pool/src/daemon/wiring.rs` | Dual-canary verification |
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
