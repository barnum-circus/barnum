# Inotify Race Condition Analysis

## The Problem

Tests pass on macOS but fail (hang) on Linux due to a race condition in `inotify`.

**The race:** When a new directory is created, there's a window between receiving the CREATE event and inotify adding a watch for that directory. Files written during this window are missed.

**Affected:** Submissions (submitter creates directory, immediately writes request file).

**Not affected:** Agents (agent creates directory, waits for daemon to write task, then writes response—causal chain guarantees watch is active).

---

## Implementation Plan

Four phases:

1. **Canary sync** - Ensure watchers are active at startup
2. **Flatten submissions** - Fix the race condition (priority: unblocks CI)
3. **Rename things** - Clean up naming (`pending/` → `submissions/`)
4. **Status file** - Proper readiness signaling for submitters (nice-to-have)

Future work (separate doc): Flatten agents + anonymous worker model. See `ANONYMOUS_WORKERS.md`.

### Naming Convention (Final State)

**Submissions (in `submissions/`):**
- `<id>.request.json` - submitter writes
- `<id>.response.json` - daemon writes

**Agents (in `agents/`, unchanged for now):**
- `<name>/task.json` - daemon writes
- `<name>/response.json` - agent writes

---

## Phase 1: Canary Sync for Both Directories

**Goal:** Ensure both `pending/` and `agents/` are watched before proceeding. Panic on non-FS events.

### 1.1: Replace `sync_with_watcher` to handle both directories

**File:** `crates/agent_pool/src/daemon/wiring.rs`

**Before (single directory):**
```rust
fn sync_with_watcher(canary_path: &Path, io_rx: &mpsc::Receiver<IoEvent>) -> io::Result<()> {
    const POLL_INTERVAL: Duration = Duration::from_millis(10);
    const ROUND_DURATION: Duration = Duration::from_millis(100);
    const MAX_ATTEMPTS: u32 = 50;

    for attempt in 0..MAX_ATTEMPTS {
        fs::write(canary_path, format!("sync-{attempt}"))?;

        let round_start = std::time::Instant::now();
        while round_start.elapsed() < ROUND_DURATION {
            match io_rx.recv_timeout(POLL_INTERVAL) {
                Ok(IoEvent::Fs(event)) => {
                    if event.paths.iter().any(|p| p == canary_path) {
                        let _ = fs::remove_file(canary_path);
                        return Ok(());
                    }
                }
                Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = fs::remove_file(canary_path);
                    return Err(io::Error::new(
                        io::ErrorKind::BrokenPipe,
                        "channel disconnected",
                    ));
                }
            }
        }
    }

    let _ = fs::remove_file(canary_path);
    Err(io::Error::new(io::ErrorKind::TimedOut, "watcher sync timed out"))
}
```

**After (both directories in parallel, panic on non-FS):**
```rust
fn sync_with_watcher(
    pending_dir: &Path,
    agents_dir: &Path,
    io_rx: &mpsc::Receiver<IoEvent>,
) -> io::Result<()> {
    const POLL_TIMEOUT: Duration = Duration::from_millis(100);
    const MAX_DURATION: Duration = Duration::from_secs(5);

    let pending_canary = pending_dir.join("canary");
    let agents_canary = agents_dir.join("canary");

    let mut seen_pending = false;
    let mut seen_agents = false;
    let mut write_count = 0u32;

    // Initial write
    fs::write(&pending_canary, format!("sync-{write_count}"))?;
    fs::write(&agents_canary, format!("sync-{write_count}"))?;
    write_count += 1;

    let start = Instant::now();
    while start.elapsed() < MAX_DURATION {
        match io_rx.recv_timeout(POLL_TIMEOUT) {
            Ok(IoEvent::Fs(event)) => {
                for path in &event.paths {
                    if path == &pending_canary { seen_pending = true; }
                    if path == &agents_canary { seen_agents = true; }
                }
                if seen_pending && seen_agents {
                    let _ = fs::remove_file(&pending_canary);
                    let _ = fs::remove_file(&agents_canary);
                    return Ok(());
                }
            }
            Ok(_) => panic!("unexpected non-FS event during startup sync"),
            Err(RecvTimeoutError::Timeout) => {
                // Rewrite only the ones we haven't seen
                if !seen_pending {
                    fs::write(&pending_canary, format!("sync-{write_count}"))?;
                }
                if !seen_agents {
                    fs::write(&agents_canary, format!("sync-{write_count}"))?;
                }
                write_count += 1;
            }
            Err(RecvTimeoutError::Disconnected) => {
                let _ = fs::remove_file(&pending_canary);
                let _ = fs::remove_file(&agents_canary);
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "channel disconnected",
                ));
            }
        }
    }

    let _ = fs::remove_file(&pending_canary);
    let _ = fs::remove_file(&agents_canary);
    Err(io::Error::new(io::ErrorKind::TimedOut, "watcher sync timed out"))
}
```

Note: The duplicated `remove_file` calls could be cleaned up with a guard/defer pattern during implementation.

### 1.2: Update call sites

**File:** `crates/agent_pool/src/daemon/wiring.rs` (around line 194 in `spawn_with_config`)

**Before:**
```rust
let canary_path = pending_dir.join(".watcher-ready");
if let Err(e) = sync_with_watcher(&canary_path, &io_rx) {
    let _ = ready_tx.send(Err(e));
    return Err(io::Error::other("watcher sync failed"));
}
```

**After:**
```rust
if let Err(e) = sync_with_watcher(&pending_dir, &agents_dir, &io_rx) {
    let _ = ready_tx.send(Err(e));
    return Err(io::Error::other("watcher sync failed"));
}
```

**Also update:** The `run_with_config` path (around line 279) with the same change.

---

## Phase 2: Flatten Submissions Directory

**Goal:** Eliminate race by using flat files. No directory creation = no new watches needed.

### 2.1: Add constants for flat file suffixes

**File:** `crates/agent_pool/src/constants.rs`

**Before:**
```rust
pub const TASK_FILE: &str = "task.json";
pub const RESPONSE_FILE: &str = "response.json";
```

**After:**
```rust
pub const TASK_FILE: &str = "task.json";
pub const RESPONSE_FILE: &str = "response.json";

// Flat file suffixes for submissions
pub const REQUEST_SUFFIX: &str = ".request.json";
pub const RESPONSE_SUFFIX: &str = ".response.json";
```

### 2.2: Update submit_file.rs

**File:** `crates/agent_pool/src/client/submit_file.rs`

**Before:**
```rust
// Generate unique submission ID
let submission_id = Uuid::new_v4().to_string();
let submission_dir = pending_dir.join(&submission_id);

// Create submission directory
fs::create_dir(&submission_dir)?;

let task_path = submission_dir.join(PENDING_TASK_FILE);
let response_path = submission_dir.join(PENDING_RESPONSE_FILE);

// Write task file with serialized payload
let content = serde_json::to_string(payload)
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
fs::write(&task_path, content)?;
```

**After:**
```rust
use crate::constants::{REQUEST_SUFFIX, RESPONSE_SUFFIX};

// Generate unique submission ID
let submission_id = Uuid::new_v4().to_string();

// Flat files directly in pending directory
let request_path = pending_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
let response_path = pending_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));

// Write request file with serialized payload (no directory creation!)
let content = serde_json::to_string(payload)
    .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
fs::write(&request_path, content)?;
```

**Cleanup (before):**
```rust
let _ = fs::remove_dir_all(&submission_dir);
```

**Cleanup (after):**
```rust
let _ = fs::remove_file(&request_path);
let _ = fs::remove_file(&response_path);
```

**Update `cleanup_submission`:**

**Before:**
```rust
pub fn cleanup_submission(root: impl AsRef<Path>, submission_id: &str) -> io::Result<()> {
    let submission_dir = root.as_ref().join(PENDING_DIR).join(submission_id);
    if submission_dir.exists() {
        fs::remove_dir_all(&submission_dir)?;
    }
    Ok(())
}
```

**After:**
```rust
pub fn cleanup_submission(root: impl AsRef<Path>, submission_id: &str) -> io::Result<()> {
    let pending_dir = root.as_ref().join(PENDING_DIR);
    let request_path = pending_dir.join(format!("{submission_id}{REQUEST_SUFFIX}"));
    let response_path = pending_dir.join(format!("{submission_id}{RESPONSE_SUFFIX}"));
    let _ = fs::remove_file(&request_path);
    let _ = fs::remove_file(&response_path);
    Ok(())
}
```

### 2.3: Update PathCategory

**File:** `crates/agent_pool/src/daemon/path_category.rs`

**Before:**
```rust
pub(super) enum PathCategory {
    AgentDir { name: String },
    AgentResponse { name: String },
    PendingDir { uuid: String },
    PendingTask { uuid: String },
}
```

**After:**
```rust
pub(super) enum PathCategory {
    AgentDir { name: String },
    AgentResponse { name: String },
    /// Submission request file: `pending/<id>.request.json`
    SubmissionRequest { id: String },
    /// Submission response file: `pending/<id>.response.json` (daemon writes, ignored)
    SubmissionResponse { id: String },
}
```

**Update `categorize_under_pending`:**

**Before:**
```rust
fn categorize_under_pending(path: &Path, pending_dir: &Path) -> Option<PathCategory> {
    let relative = path.strip_prefix(pending_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    if components.is_empty() {
        return None;
    }

    let uuid = components[0].as_os_str().to_str()?.to_string();

    match components.len() {
        1 => Some(PathCategory::PendingDir { uuid }),
        2 => {
            let filename = components[1].as_os_str().to_str()?;
            if filename == TASK_FILE {
                Some(PathCategory::PendingTask { uuid })
            } else {
                None
            }
        }
        _ => None,
    }
}
```

**After:**
```rust
use crate::constants::{REQUEST_SUFFIX, RESPONSE_SUFFIX};

fn categorize_under_pending(path: &Path, pending_dir: &Path) -> Option<PathCategory> {
    let relative = path.strip_prefix(pending_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    // Must be exactly one component (flat file)
    if components.len() != 1 {
        return None;
    }

    let filename = components[0].as_os_str().to_str()?;

    if let Some(id) = filename.strip_suffix(REQUEST_SUFFIX) {
        return Some(PathCategory::SubmissionRequest { id: id.to_string() });
    }

    if let Some(id) = filename.strip_suffix(RESPONSE_SUFFIX) {
        return Some(PathCategory::SubmissionResponse { id: id.to_string() });
    }

    None
}
```

### 2.4: Update wiring.rs event handling

**File:** `crates/agent_pool/src/daemon/wiring.rs` (handle_fs_event, around line 557)

**Before:**
```rust
PathCategory::PendingDir { uuid } => {
    debug!(uuid = %uuid, "PendingDir: ignoring directory event");
}
PathCategory::PendingTask { uuid } => {
    let submission_dir = pending_dir.join(&uuid);
    if path.exists() {
        register_pending_task(
            &submission_dir,
            events_tx,
            external_task_map,
            task_id_allocator,
            io_config,
        );
    }
}
```

**After:**
```rust
PathCategory::SubmissionRequest { id } => {
    assert!(path.exists(), "SubmissionRequest event for non-existent path: {path:?}");
    register_submission(
        &id,
        pending_dir,
        events_tx,
        external_task_map,
        task_id_allocator,
        io_config,
    );
}
PathCategory::SubmissionResponse { id } => {
    // Daemon writes these, ignore our own writes
    trace!(id = %id, "SubmissionResponse: ignoring (daemon wrote this)");
}
```

### 2.5: Update register_pending_task → register_submission

**File:** `crates/agent_pool/src/daemon/wiring.rs`

**Before:**
```rust
fn register_pending_task(
    submission_dir: &Path,
    events_tx: &mpsc::Sender<Event>,
    external_task_map: &mut ExternalTaskMap,
    task_id_allocator: &mut TaskIdAllocator,
    io_config: &IoConfig,
) {
    let task_path = submission_dir.join(TASK_FILE);
    let response_path = submission_dir.join(crate::constants::RESPONSE_FILE);

    // Already registered?
    if let Some(existing_id) = external_task_map.get_id_by_path(submission_dir) {
        // ...
    }

    // Already completed? (response.json exists)
    if response_path.exists() {
        // ...
    }

    // Read and resolve payload
    let raw = match fs::read_to_string(&task_path) {
        // ...
    };

    // Register the task
    let external_id = task_id_allocator.allocate_external();
    if external_task_map.register(
        external_id,
        submission_dir.to_path_buf(),  // stores directory path
        ExternalTaskData { ... },
    ) {
        // ...
    }
}
```

**After:**
```rust
fn register_submission(
    id: &str,
    pending_dir: &Path,
    events_tx: &mpsc::Sender<Event>,
    external_task_map: &mut ExternalTaskMap,
    task_id_allocator: &mut TaskIdAllocator,
    io_config: &IoConfig,
) {
    let request_path = pending_dir.join(format!("{id}{REQUEST_SUFFIX}"));
    let response_path = pending_dir.join(format!("{id}{RESPONSE_SUFFIX}"));

    // Duplicate FS event - skip silently
    if external_task_map.get_id_by_path(&request_path).is_some() {
        trace!(id = %id, "SubmissionRequest: duplicate event, skipping");
        return;
    }

    // Already completed? This shouldn't happen
    assert!(
        !response_path.exists(),
        "SubmissionRequest for already-completed submission: {id}"
    );

    // Read and resolve payload
    let raw = match fs::read_to_string(&request_path) {
        // ...
    };

    // Register the submission
    let external_id = task_id_allocator.allocate_external();
    external_task_map.register(
        external_id,
        request_path,  // stores request file path
        ExternalTaskData { ... },
    );
    // ...
}
```

### 2.6: Update ExternalTaskMap.finish()

**File:** `crates/agent_pool/src/daemon/io.rs`

**Note:** `Transport::Directory(path)` now stores a file path (the request file), not a directory. This is awkward naming. Ideally we'd store just the ID and derive paths, or rename to `Transport::File`. However, since `TransportMap` is shared with agents (which still use directories), changing this now adds complexity. Revisit when implementing the anonymous worker model (see `ANONYMOUS_WORKERS.md`).

**Before:**
```rust
Transport::Directory(path) => {
    debug!(
        external_task_id = id.0,
        path = %path.display(),
        "finish: writing response.json"
    );
    fs::write(path.join(RESPONSE_FILE), response)?;
}
```

**After:**
```rust
Transport::Directory(path) => {
    // path is the request file; derive response path
    let response_path = path.with_file_name(
        path.file_name()
            .and_then(|n| n.to_str())
            .and_then(|n| n.strip_suffix(REQUEST_SUFFIX))
            .map(|id| format!("{id}{RESPONSE_SUFFIX}"))
            .expect("request path should have REQUEST_SUFFIX")
    );
    debug!(
        external_task_id = id.0,
        path = %response_path.display(),
        "finish: writing response"
    );
    fs::write(response_path, response)?;
}
```

### 2.7: Update tests

Update all tests in `path_category.rs` and `wiring.rs` that reference the old directory structure.

---

## Phase 3: Rename Things

### 3.1: Rename pending/ → submissions/

**File:** `crates/agent_pool/src/constants.rs`

**Before:**
```rust
pub const PENDING_DIR: &str = "pending";
```

**After:**
```rust
pub const SUBMISSIONS_DIR: &str = "submissions";
```

### 3.2: Rename variables throughout

- `pending_dir` → `submissions_dir`
- Update all references in `wiring.rs`, `submit_file.rs`, etc.
- Update log messages

### 3.3: Update documentation

- `SUBMISSION_PROTOCOL.md`

---

## Phase 4: Status File for Readiness

**Goal:** Proper synchronization so submitters know the daemon is truly ready.

### 4.1: Daemon writes status file after canary sync

**File:** `crates/agent_pool/src/daemon/wiring.rs`

After canary sync completes successfully:

```rust
// Write status file at pool root
fs::write(root.join("status"), "ready")?;
```

### 4.2: Submitter waits for status file

**File:** `crates/agent_pool/src/client/submit_file.rs`

**Before:**
```rust
// Daemon creates pending_dir after watcher starts - if it doesn't exist, daemon isn't ready
if !pending_dir.exists() {
    return Err(io::Error::new(
        io::ErrorKind::NotConnected,
        "daemon not ready (pending directory doesn't exist)",
    ));
}
```

**After:**
```rust
// Wait for daemon to be ready
let status_path = root.join("status");
let start = Instant::now();
while !status_path.exists() {
    if start.elapsed() > timeout {
        return Err(io::Error::new(
            io::ErrorKind::NotConnected,
            "daemon not ready (timed out waiting for status file)",
        ));
    }
    thread::sleep(POLL_INTERVAL);
}

let status = fs::read_to_string(&status_path)?;
if status.trim() != "ready" {
    return Err(io::Error::new(
        io::ErrorKind::NotConnected,
        format!("daemon not ready (status: {})", status.trim()),
    ));
}
```

### 4.3: Add constant

**File:** `crates/agent_pool/src/constants.rs`

```rust
pub const STATUS_FILE: &str = "status";
```

### Future status values

For now, just "ready". Later could add:
- "paused" - not accepting new submissions
- "shutting_down" - draining existing work
- etc.

---

---

## Phase 5: Clean Shutdown

**Goal:** Clear pool directory on shutdown/stop. State is in-memory; no partial recovery is possible.

### 5.1: On daemon shutdown

Delete the entire pool directory contents (or the directory itself):
- `submissions/` (or `pending/`)
- `agents/`
- `status`
- Any orphaned canary files

### 5.2: On `agent_pool stop`

Kill the daemon PID and delete the pool directory. Order doesn't matter.

### 5.3: On startup

If pool directory exists with stale files from a crashed daemon, clean them up before proceeding.

---

## Task Order

1. **Phase 1** (canary sync) - Small, independent
2. **Phase 2** (flatten submissions) - **Push after this to fix CI**
3. **Phase 3** (rename things) - Cleanup
4. **Phase 4** (status file) - Nice-to-have, doesn't break tests
5. **Phase 5** (clean shutdown) - Can be done before or after other phases

Future: See `ANONYMOUS_WORKERS.md` for flattening agents + anonymous worker model.
