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

### Core Abstraction

```rust
/// A file watcher that verifies itself via canary file.
///
/// The watcher can optionally short-circuit verification if a target file
/// is seen before the canary - seeing any event proves the watcher works.
pub struct VerifiedWatcher {
    rx: mpsc::Receiver<PathBuf>,
    _watcher: RecommendedWatcher,
}

impl VerifiedWatcher {
    /// Create a verified watcher on a directory.
    ///
    /// Writes a canary file and waits for either:
    /// - The canary event (watcher verified)
    /// - An event for `short_circuit_on` file (implicitly verifies watcher)
    ///
    /// # Arguments
    ///
    /// * `watch_dir` - Directory to watch (must exist)
    /// * `canary_path` - Path for canary file (will be created and deleted)
    /// * `short_circuit_on` - Optional file that implicitly verifies watcher if seen
    /// * `verify_timeout` - How long to wait for verification (None = wait forever)
    ///
    /// # Returns
    ///
    /// Returns `(watcher, short_circuited)` where `short_circuited` is true if
    /// the target file was seen during verification.
    pub fn new(
        watch_dir: &Path,
        canary_path: &Path,
        short_circuit_on: Option<&Path>,
        verify_timeout: Option<Duration>,
    ) -> io::Result<(Self, bool)>;

    /// Wait for a specific file to appear.
    ///
    /// # Arguments
    ///
    /// * `target` - File to wait for
    /// * `timeout` - How long to wait (`None` = wait forever, used by GSD for indefinite waits)
    pub fn wait_for(&self, target: &Path, timeout: Option<Duration>) -> io::Result<()>;

    /// Get the raw event receiver for custom event processing.
    pub fn into_receiver(self) -> mpsc::Receiver<PathBuf>;
}
```

### Key Insight: Short-Circuit Verification

If we're waiting for file X and doing canary verification:
- See canary event → watcher verified, continue waiting for X
- See X event → watcher implicitly verified (we saw an event!), return immediately

This eliminates the `recv_timeout` loop once we've seen ANY event.

---

## Use Cases

### Overview

| Use Case | Flow | Caller | Description |
|----------|------|--------|-------------|
| `submit_file` | **Client → Daemon** | CLI/SDK submitting tasks | Wait for pool ready, then response |
| `wait_for_pool_ready` | **Client → Daemon** | CLI/SDK checking daemon | Wait for status file (daemon alive) |
| Daemon startup | **Daemon init** | Daemon process | Verify watchers before accepting work |
| Agent task wait | **Agent → Daemon** | Agent processes | Wait for task assignment |

---

### 1. File-Based Submission (`submit_file`)

**Flow: Client → Daemon** (submitter waiting for task completion)

**Current flow (two watchers):**
```rust
pub fn submit_file_with_timeout(...) -> io::Result<Response> {
    // WATCHER #1: wait for pool ready
    wait_for_pool_ready(root, DEFAULT_POOL_READY_TIMEOUT)?;

    // ... write request ...

    // WATCHER #2: wait for response (in new implementation)
    let watcher = create_watcher(...)?;
    // ... wait for response ...
}
```

**New flow (one watcher):**
```rust
pub fn submit_file_with_timeout(
    root: impl AsRef<Path>,
    payload: &Payload,
    timeout: Option<Duration>,  // None = wait forever (GSD use case)
) -> io::Result<Response> {
    let root = fs::canonicalize(root.as_ref())?;
    let submissions_dir = root.join(SUBMISSIONS_DIR);
    let submission_id = Uuid::new_v4().to_string();

    let request_path = submissions_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = submissions_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));
    let canary_path = submissions_dir.join(format!("{submission_id}.canary"));
    let status_path = root.join(STATUS_FILE);

    // Single watcher on submissions/, short-circuit on status file
    let (watcher, status_seen) = VerifiedWatcher::new(
        &submissions_dir,
        &canary_path,
        Some(&status_path),  // Short-circuit if we see status file
        Some(Duration::from_secs(5)),  // Verification timeout
    )?;

    // If status file wasn't seen during verification, wait for it
    if !status_seen {
        watcher.wait_for(&status_path, Some(Duration::from_secs(10)))?;
    }

    // Now we know: watcher works AND pool is ready
    // Write request file
    atomic_write_str(&root, &request_path, &serde_json::to_string(payload)?)?;

    // Wait for response (None = wait forever, like GSD does)
    watcher.wait_for(&response_path, timeout)?;  // timeout is Option<Duration>

    // Read and cleanup
    read_and_cleanup_response(&request_path, &response_path)
}
```

**Key points:**
- One watcher created, used for both status file and response file
- If status file event seen during canary verification, skip waiting for it
- Watcher is already verified when we write the request

### 2. Wait for Pool Ready (`wait_for_pool_ready`)

**Flow: Client → Daemon** (verifying daemon is alive before submission)

**Current implementation:**
```rust
pub fn wait_for_pool_ready(root: impl AsRef<Path>, timeout: Duration) -> io::Result<()> {
    // ... 120 lines of watcher setup, canary verification, status file wait ...
}
```

**New implementation:**
```rust
pub fn wait_for_pool_ready(root: impl AsRef<Path>, timeout: Option<Duration>) -> io::Result<()> {
    let root = fs::canonicalize(root.as_ref())?;
    let status_path = root.join(STATUS_FILE);
    let canary_path = root.join("client_canary");

    // Short-circuit on status file - if we see it, watcher is implicitly verified
    let (watcher, status_seen) = VerifiedWatcher::new(
        &root,
        &canary_path,
        Some(&status_path),
        timeout,  // None = wait forever
    )?;

    if status_seen {
        // Status file seen during verification - pool is ready
        return Ok(());
    }

    // Watcher verified, now wait for status file
    watcher.wait_for(&status_path, timeout)
}
```

**Key points:**
- Much simpler: ~10 lines instead of ~120
- If status file already exists and we see the event, return immediately
- Otherwise wait for it with verified watcher

### 3. Daemon Startup (`daemon/wiring.rs`)

**Flow: Daemon init** (verifying filesystem watchers work before accepting connections)

**Current implementation:**
```rust
// In sync_and_setup():
// - Creates watcher on pool root
// - Creates agents/, submissions/, canary
// - Waits to see events for all created items
// - Complex logic to track which events we've seen
```

**New implementation:**

The daemon needs to verify watchers on TWO directories:
- `agents/` - for agent registration events
- `submissions/` - for file-based submission events

```rust
fn verify_daemon_watchers(
    root: &Path,
    agents_dir: &Path,
    submissions_dir: &Path,
    io_rx: &mpsc::Receiver<IoEvent>,
) -> io::Result<()> {
    // Create both directories first
    fs::create_dir_all(agents_dir)?;
    fs::create_dir_all(submissions_dir)?;

    // Canary in each directory
    let agents_canary = agents_dir.join("daemon.canary");
    let submissions_canary = submissions_dir.join("daemon.canary");

    // Write both canaries
    fs::write(&agents_canary, "sync")?;
    fs::write(&submissions_canary, "sync")?;

    // Wait for both canary events (already have watcher from earlier setup)
    let mut seen_agents = false;
    let mut seen_submissions = false;
    let start = Instant::now();
    let timeout = Duration::from_secs(5);

    while !seen_agents || !seen_submissions {
        if start.elapsed() > timeout {
            // Retry canaries
            if !seen_agents {
                fs::write(&agents_canary, start.elapsed().as_millis().to_string())?;
            }
            if !seen_submissions {
                fs::write(&submissions_canary, start.elapsed().as_millis().to_string())?;
            }
        }

        match io_rx.recv_timeout(Duration::from_millis(100)) {
            Ok(IoEvent::Fs(event)) => {
                for path in &event.paths {
                    if path == &agents_canary {
                        seen_agents = true;
                        let _ = fs::remove_file(&agents_canary);
                    } else if path == &submissions_canary {
                        seen_submissions = true;
                        let _ = fs::remove_file(&submissions_canary);
                    }
                }
            }
            Ok(_) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if start.elapsed() > Duration::from_secs(30) {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "daemon watcher verification timed out",
                    ));
                }
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher channel disconnected",
                ));
            }
        }
    }

    Ok(())
}
```

**Key points:**
- Two canary files, one per watched directory
- Watcher already created earlier in daemon startup
- Verifies both directories before entering main loop

### 4. Agent Waiting for Task (`agent.rs`)

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

    // Short-circuit on task file - if we see it, read it immediately
    let (watcher, task_seen) = VerifiedWatcher::new(
        agent_dir,
        &canary_path,
        Some(&task_path),
        Some(Duration::from_secs(5)),  // Verification timeout
    )?;

    if task_seen {
        // Task file event seen during verification
        return fs::read_to_string(&task_path);
    }

    // Watcher verified, wait for task (None = wait forever for long-running agents)
    watcher.wait_for(&task_path, timeout)?;
    fs::read_to_string(&task_path)
}
```

**Key points:**
- If task.json arrives during canary verification, return immediately
- Single watcher, single verification

---

## Implementation Plan

### Step 1: Create `VerifiedWatcher` in `fs_util.rs`

Add to existing `fs_util.rs`:

```rust
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// A file watcher verified via canary file.
pub struct VerifiedWatcher {
    rx: mpsc::Receiver<PathBuf>,
    _watcher: RecommendedWatcher,
}

impl VerifiedWatcher {
    /// Create a new verified watcher.
    ///
    /// # Arguments
    ///
    /// * `watch_dir` - Directory to watch
    /// * `canary_path` - Path for canary file (created and deleted during verification)
    /// * `short_circuit_on` - If this file is seen, return early (implicitly verifies watcher)
    /// * `verify_timeout` - Timeout for canary verification (None = wait forever)
    ///
    /// # Returns
    ///
    /// `(watcher, short_circuited)` - watcher and whether short-circuit file was seen
    pub fn new(
        watch_dir: &Path,
        canary_path: &Path,
        short_circuit_on: Option<&Path>,
        verify_timeout: Option<Duration>,
    ) -> io::Result<(Self, bool)> {
        let (tx, rx) = mpsc::channel();
        let watch_dir_for_closure = watch_dir.to_path_buf();

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

        // Write canary
        fs::write(canary_path, "sync")?;

        let start = Instant::now();
        let mut short_circuited = false;

        // Wait for canary or short-circuit target
        loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(path) => {
                    if path == canary_path {
                        // Canary seen - watcher verified
                        let _ = fs::remove_file(canary_path);
                        break;
                    } else if short_circuit_on.is_some_and(|target| path == target) {
                        // Target file seen - implicitly verified
                        let _ = fs::remove_file(canary_path);
                        short_circuited = true;
                        break;
                    }
                    // Other event - continue waiting
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(timeout) = verify_timeout {
                        if start.elapsed() > timeout {
                            let _ = fs::remove_file(canary_path);
                            return Err(io::Error::new(
                                io::ErrorKind::TimedOut,
                                "watcher verification timed out",
                            ));
                        }
                    }
                    // Rewrite canary to trigger event
                    fs::write(canary_path, start.elapsed().as_millis().to_string())?;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = fs::remove_file(canary_path);
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "watcher disconnected",
                    ));
                }
            }
        }

        Ok((Self { rx, _watcher: watcher }, short_circuited))
    }

    /// Wait for a specific file to appear.
    ///
    /// Pass `None` for timeout to wait forever (used by GSD for indefinite waits).
    pub fn wait_for(&self, target: &Path, timeout: Option<Duration>) -> io::Result<()> {
        // Check if already exists
        if target.exists() {
            return Ok(());
        }

        let start = Instant::now();

        loop {
            // Check timeout if specified
            if let Some(t) = timeout {
                let remaining = t.saturating_sub(start.elapsed());
                if remaining.is_zero() {
                    // Final check
                    if target.exists() {
                        return Ok(());
                    }
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        format!("timed out waiting for {}", target.display()),
                    ));
                }
            }

            match self.rx.recv_timeout(Duration::from_millis(100)) {
                Ok(path) if path == target => return Ok(()),
                Ok(_) => {
                    // Different file - check target anyway
                    if target.exists() {
                        return Ok(());
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if target.exists() {
                        return Ok(());
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

    /// Consume the watcher and return the raw event receiver.
    ///
    /// Use this when you need custom event processing (e.g., daemon main loop).
    pub fn into_receiver(self) -> mpsc::Receiver<PathBuf> {
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
