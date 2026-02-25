# Inotify Race Condition Analysis

## The Problem

Tests pass on macOS but fail (hang) on Linux. The root cause is a race condition inherent to `inotify` that doesn't exist in `FSEvents`.

### FSEvents vs Inotify

**macOS FSEvents:**
- Directory-level monitoring
- Watches entire tree automatically
- No race when subdirectories are created

**Linux inotify:**
- Per-directory watches
- Must manually add watches for new subdirectories
- **Race condition**: When a new directory is created, there's a window between:
  1. Receiving the CREATE event for the directory
  2. Adding a watch for that directory
- Files written during this window are missed

## Where The Race Occurs

### 1. Pending Task Submission (`NotifyMethod::Raw`)

```
1. Submitter creates `pending/<uuid>/`
2. inotify receives CREATE event
3. notify crate tries to add watch for `pending/<uuid>/`
4. Submitter writes `task.json`
5. If (4) happens before (3) completes, we miss the PendingTask event
```

### 2. Agent Response Detection

```
1. Agent creates `agents/<name>/`
2. inotify receives CREATE event
3. notify crate tries to add watch for `agents/<name>/`
4. Daemon writes `task.json` (heartbeat)
5. Agent reads `task.json`, writes `response.json`
6. If (5) happens before (3) completes, we miss the AgentResponse event
```

## Why Simple Fallbacks Don't Work

The initial "fix" was to check if `task.json` exists when we see a `PendingDir` event:

```rust
// BROKEN - This doesn't work!
PathCategory::PendingDir { uuid } => {
    let task_path = pending_dir.join(&uuid).join(TASK_FILE);
    if task_path.exists() {
        register_pending_task(...);
    }
}
```

**The flaw:** When we receive the `PendingDir` event, the watch might not be set up yet. If `task.json` doesn't exist at that moment, we return and expect to catch it later via `PendingTask`. But if the watch isn't ready, we'll never see `PendingTask` either.

## The Correct Solution: Canary-Based Synchronization

We already have this pattern at daemon startup in `sync_with_watcher()`:

```rust
// From wiring.rs:945-987
fn sync_with_watcher(canary_path: &Path, io_rx: &mpsc::Receiver<IoEvent>) -> io::Result<()> {
    const POLL_INTERVAL: Duration = Duration::from_millis(10);
    const ROUND_DURATION: Duration = Duration::from_millis(100);
    const MAX_ATTEMPTS: u32 = 50;

    for attempt in 0..MAX_ATTEMPTS {
        // Write canary with unique content to trigger new FS event each round
        fs::write(canary_path, format!("sync-{attempt}"))?;

        let round_start = std::time::Instant::now();
        while round_start.elapsed() < ROUND_DURATION {
            match io_rx.recv_timeout(POLL_INTERVAL) {
                Ok(IoEvent::Fs(event)) => {
                    if event.paths.iter().any(|p| p == canary_path) {
                        let _ = fs::remove_file(canary_path);
                        return Ok(());
                    }
                    // Not our canary, keep polling
                }
                // ... timeout handling
            }
        }
        // Retry with new content
    }
    // ... error handling
}
```

**The key insight:** By writing a file and waiting for its FS event, we *prove* the watch is active. If we see the canary event, we know any subsequent writes will also be seen.

## Implementation Plan

### Goal

When we see a `PendingDir` or `AgentDir` event, sync the watcher for that specific subdirectory before checking for `task.json` or `response.json`.

### Challenge

The existing `sync_with_watcher` consumes events from `io_rx`. During the event loop, this would cause us to drop other events. We need to **buffer** any non-canary events encountered during the sync and re-process them afterward.

### Design

1. **Extract canary sync into a reusable function** that buffers non-canary events
2. **Integrate into the I/O loop** for `PendingDir` and `AgentDir` events
3. **Use `.canary` as the filename** (not `.watcher-ready` or `.watcher-sync`)

---

## Detailed Implementation

### Step 1: Create a New Sync Function That Buffers Events

**File:** `crates/agent_pool/src/daemon/wiring.rs`

**New function:**

```rust
/// Synchronize with the filesystem watcher for a specific directory.
///
/// Writes a canary file and waits for the corresponding FS event, proving
/// the watch is active. Any non-canary events received during the sync
/// are buffered and returned so they can be re-processed.
///
/// Returns `Ok(buffered_events)` on success, `Err` on timeout or channel close.
fn sync_directory_watcher(
    dir: &Path,
    io_rx: &mpsc::Receiver<IoEvent>,
) -> io::Result<Vec<IoEvent>> {
    const POLL_INTERVAL: Duration = Duration::from_millis(10);
    const ROUND_DURATION: Duration = Duration::from_millis(100);
    const MAX_ATTEMPTS: u32 = 50; // 5s total

    let canary_path = dir.join(".canary");
    let mut buffered_events = Vec::new();

    debug!(dir = %dir.display(), "syncing directory watcher");

    for attempt in 0..MAX_ATTEMPTS {
        // Write canary with unique content to trigger new FS event each round
        fs::write(&canary_path, format!("sync-{attempt}"))?;

        let round_start = std::time::Instant::now();
        while round_start.elapsed() < ROUND_DURATION {
            match io_rx.recv_timeout(POLL_INTERVAL) {
                Ok(IoEvent::Fs(event)) => {
                    if event.paths.iter().any(|p| p == &canary_path) {
                        debug!(attempt, dir = %dir.display(), "directory watcher sync complete");
                        let _ = fs::remove_file(&canary_path);
                        return Ok(buffered_events);
                    }
                    // Not our canary - buffer it for later processing
                    buffered_events.push(IoEvent::Fs(event));
                }
                Ok(other) => {
                    // Non-FS event (Effect, Socket, etc.) - buffer it
                    buffered_events.push(other);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    // Keep polling
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = fs::remove_file(&canary_path);
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "watcher channel disconnected during directory sync",
                    ));
                }
            }
        }
        // Round finished without seeing canary, retry with new content
    }

    let _ = fs::remove_file(&canary_path);
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        format!("directory watcher sync timed out for {}", dir.display()),
    ))
}
```

### Step 2: Refactor Startup Sync to Use the New Function

**Before (current code at lines 194-198):**

```rust
let canary_path = pending_dir.join(".watcher-ready");
if let Err(e) = sync_with_watcher(&canary_path, &io_rx) {
    let _ = ready_tx.send(Err(e));
    return Err(io::Error::other("watcher sync failed"));
}
```

**After:**

```rust
// Sync the pending directory watcher (canary events at startup are discarded)
if let Err(e) = sync_directory_watcher(pending_dir, &io_rx) {
    let _ = ready_tx.send(Err(e));
    return Err(io::Error::other("watcher sync failed"));
}
// Note: buffered events discarded at startup - nothing important yet
```

Also update line 279-280 similarly.

### Step 3: Delete the Old `sync_with_watcher` Function

Remove lines 933-987 (the old function). The new `sync_directory_watcher` replaces it.

### Step 4: Update PathCategory to Ignore Canary Files

**File:** `crates/agent_pool/src/daemon/path_category.rs`

We need to ensure `.canary` files don't match any category (they should be ignored).

**Find the categorization logic and add:**

```rust
// Early return for canary files - they're internal sync mechanism
if path.file_name() == Some(std::ffi::OsStr::new(".canary")) {
    return None;
}
```

### Step 5: Modify I/O Loop to Sync on PendingDir Events

**File:** `crates/agent_pool/src/daemon/wiring.rs`

The I/O loop needs access to `io_rx` to call the sync function. Currently `handle_fs_event` doesn't have access to it.

**Current signature (line 511):**

```rust
fn handle_fs_event(
    event: &notify::Event,
    events_tx: &mpsc::Sender<Event>,
    agent_map: &mut AgentMap,
    external_task_map: &mut ExternalTaskMap,
    task_id_allocator: &mut TaskIdAllocator,
    pending_responses: &mut HashSet<AgentId>,
    kicked_paths: &mut HashSet<PathBuf>,
    agents_dir: &Path,
    pending_dir: &Path,
    io_config: &IoConfig,
)
```

**Option A: Move sync logic into io_loop**

Instead of passing `io_rx` through many functions, handle the sync in `io_loop` before calling `handle_fs_event`:

**Current io_loop (simplified):**

```rust
while let Ok(io_event) = io_rx.recv() {
    match io_event {
        IoEvent::Fs(event) => {
            handle_fs_event(&event, ...);
        }
        // ...
    }
}
```

**After:**

```rust
while let Ok(io_event) = io_rx.recv() {
    match io_event {
        IoEvent::Fs(event) => {
            // Check if this is a new directory that needs sync
            let buffered = sync_new_directories_if_needed(
                &event,
                agents_dir,
                pending_dir,
                &io_rx,
            );

            // Process any buffered events first
            for buffered_event in buffered {
                process_io_event(buffered_event, ...);
            }

            // Now process the original event
            handle_fs_event(&event, ...);
        }
        // ...
    }
}
```

**New helper function:**

```rust
/// If the event contains new PendingDir or AgentDir paths, sync them.
/// Returns any events buffered during the sync.
fn sync_new_directories_if_needed(
    event: &notify::Event,
    agents_dir: &Path,
    pending_dir: &Path,
    io_rx: &mpsc::Receiver<IoEvent>,
) -> Vec<IoEvent> {
    let mut all_buffered = Vec::new();

    for path in &event.paths {
        let Some(category) = path_category::categorize(path, agents_dir, pending_dir) else {
            continue;
        };

        match category {
            PathCategory::PendingDir { ref uuid } => {
                let submission_dir = pending_dir.join(uuid);
                if submission_dir.is_dir() {
                    match sync_directory_watcher(&submission_dir, io_rx) {
                        Ok(buffered) => all_buffered.extend(buffered),
                        Err(e) => warn!(error = %e, uuid = %uuid, "failed to sync pending dir"),
                    }
                }
            }
            PathCategory::AgentDir { ref name } => {
                let agent_path = agents_dir.join(name);
                if agent_path.is_dir() {
                    match sync_directory_watcher(&agent_path, io_rx) {
                        Ok(buffered) => all_buffered.extend(buffered),
                        Err(e) => warn!(error = %e, name = %name, "failed to sync agent dir"),
                    }
                }
            }
            _ => {}
        }
    }

    all_buffered
}
```

### Step 6: Update PendingDir Handler to Check for task.json

**Current (line 557-561):**

```rust
PathCategory::PendingDir { uuid } => {
    // Directory creation events are ignored - we wait for task.json.
    // The watcher sync at startup ensures inotify is ready.
    debug!(uuid = %uuid, "PendingDir: ignoring directory event");
}
```

**After:**

```rust
PathCategory::PendingDir { uuid } => {
    // After sync_new_directories_if_needed runs, the watch is active.
    // Check if task.json already exists (written during sync window).
    let submission_dir = pending_dir.join(&uuid);
    let task_path = submission_dir.join(TASK_FILE);
    if task_path.exists() {
        debug!(uuid = %uuid, "PendingDir: task.json exists after sync, registering");
        register_pending_task(
            &submission_dir,
            events_tx,
            external_task_map,
            task_id_allocator,
            io_config,
        );
    } else {
        debug!(uuid = %uuid, "PendingDir: waiting for PendingTask event");
    }
}
```

### Step 7: Update AgentDir Handler Similarly

**Current (simplified):**

```rust
PathCategory::AgentDir { name } => {
    let agent_path = agents_dir.join(&name);
    handle_agent_dir(&agent_path, ...);
}
```

**After:** `handle_agent_dir` should check for `response.json` after the sync:

```rust
fn handle_agent_dir(
    agent_path: &Path,
    events_tx: &mpsc::Sender<Event>,
    agent_map: &mut AgentMap,
    pending_responses: &mut HashSet<AgentId>,
    kicked_paths: &mut HashSet<PathBuf>,
    task_id_allocator: &mut TaskIdAllocator,
    io_config: &IoConfig,
) {
    // ... existing registration logic ...

    if let Some(agent_id) = agent_map.register_directory(agent_path.to_path_buf(), ()) {
        // ... send AgentRegistered event ...

        // After sync, check if response.json already exists
        let response_path = agent_path.join(crate::constants::RESPONSE_FILE);
        if response_path.exists() {
            debug!(agent_id = agent_id.0, "AgentDir: response.json exists after sync");
            if pending_responses.insert(agent_id) {
                let _ = events_tx.send(Event::AgentResponded { agent_id });
            }
        }
    }
}
```

---

## Summary of Changes

| File | Change |
|------|--------|
| `wiring.rs` | Add `sync_directory_watcher()` function |
| `wiring.rs` | Delete old `sync_with_watcher()` function |
| `wiring.rs` | Update startup sync to use new function |
| `wiring.rs` | Add `sync_new_directories_if_needed()` helper |
| `wiring.rs` | Modify `io_loop` to call sync helper |
| `wiring.rs` | Update `PendingDir` handler to check task.json after sync |
| `wiring.rs` | Update `handle_agent_dir` to check response.json after sync |
| `path_category.rs` | Ignore `.canary` files in categorization |

## Why This Works

1. When we receive a `PendingDir` event, we **immediately sync** that specific directory
2. The sync **blocks** until we see the canary event (proving watch is active)
3. Any events that arrive during sync are **buffered** and processed afterward
4. After sync completes, we check if `task.json` exists (it might have been written during the race window)
5. If `task.json` doesn't exist, we'll catch it via `PendingTask` (watch is now guaranteed active)

The blocking/spinning is **encapsulated** in `sync_directory_watcher()` and doesn't leak into the rest of the codebase.
