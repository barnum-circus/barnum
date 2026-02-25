# Inotify Race Condition Analysis

## The Problem

Tests pass on macOS but fail (hang) on Linux due to a race condition in `inotify`.

**The race:** When a new directory is created, there's a window between receiving the CREATE event and inotify adding a watch for that directory. Files written during this window are missed.

**Affected:** Submissions (submitter creates directory, immediately writes request file).

**Not affected:** Agents (agent creates directory, waits for daemon to write task, then writes outcome—causal chain guarantees watch is active).

---

## Implementation Plan

Five phases:

1. **Canary sync** - Ensure watchers are active at startup
2. **Flatten submissions** - Fix the race condition (priority: unblocks CI)
3. **Flatten agents** - Consistency, reuse logic from phase 2
4. **Rename things** - Clean up naming
5. **Anonymous worker model** - Simplify agent protocol

### Naming Convention (Final State)

**Submissions (in `submissions/`):**
- `<id>.request.json` - submitter writes
- `<id>.response.json` - daemon writes

**Agents (in `agents/`):**
- `<id>.task.json` - daemon writes
- `<id>.outcome.json` - agent writes

---

## Phase 1: Canary Sync for Both Directories

**Goal:** Ensure both `pending/` and `agents/` are watched before proceeding. Panic on non-FS events.

### 1.1: Update `sync_with_watcher` to panic on non-FS events

**File:** `crates/agent_pool/src/daemon/wiring.rs` (lines 967-969)

**Before:**
```rust
Ok(_) | Err(mpsc::RecvTimeoutError::Timeout) => {
    // Non-FS event or timeout, keep polling
}
```

**After:**
```rust
Ok(IoEvent::Socket(..) | IoEvent::Effect(..) | IoEvent::Shutdown) => {
    panic!("unexpected non-FS event during startup sync");
}
Err(mpsc::RecvTimeoutError::Timeout) => {
    // Keep polling
}
```

### 1.2: Sync both directories at startup

**File:** `crates/agent_pool/src/daemon/wiring.rs` (around line 194)

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
// Sync pending directory
let pending_canary = pending_dir.join("canary");
if let Err(e) = sync_with_watcher(&pending_canary, &io_rx) {
    let _ = ready_tx.send(Err(e));
    return Err(io::Error::other("pending watcher sync failed"));
}

// Sync agents directory
let agents_canary = agents_dir.join("canary");
if let Err(e) = sync_with_watcher(&agents_canary, &io_rx) {
    let _ = ready_tx.send(Err(e));
    return Err(io::Error::other("agents watcher sync failed"));
}
```

**Also update:** The `run_with_config` path (around line 279) with the same changes.

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
    if path.exists() {
        register_submission(
            &id,
            pending_dir,
            events_tx,
            external_task_map,
            task_id_allocator,
            io_config,
        );
    }
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

    // Already registered?
    if let Some(existing_id) = external_task_map.get_id_by_path(&request_path) {
        // ...
    }

    // Already completed?
    if response_path.exists() {
        // ...
    }

    // Read and resolve payload
    let raw = match fs::read_to_string(&request_path) {
        // ...
    };

    // Register the submission
    let external_id = task_id_allocator.allocate_external();
    if external_task_map.register(
        external_id,
        request_path,  // stores request file path
        ExternalTaskData { ... },
    ) {
        // ...
    }
}
```

### 2.6: Update ExternalTaskMap.finish()

**File:** `crates/agent_pool/src/daemon/io.rs`

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

## Phase 3: Flatten Agents Directory

Similar pattern to Phase 2, but for agents.

### 3.1: Add constants

**File:** `crates/agent_pool/src/constants.rs`

**Add:**
```rust
// Flat file suffixes for agents
pub const TASK_SUFFIX: &str = ".task.json";
pub const OUTCOME_SUFFIX: &str = ".outcome.json";
```

### 3.2: Update PathCategory

**Before:**
```rust
AgentDir { name: String },
AgentResponse { name: String },
```

**After:**
```rust
/// Agent task file: `agents/<id>.task.json` (daemon writes, ignored)
AgentTask { id: String },
/// Agent outcome file: `agents/<id>.outcome.json`
AgentOutcome { id: String },
```

### 3.3: Update categorize_under_agents

**Before:**
```rust
fn categorize_under_agents(path: &Path, agents_dir: &Path) -> Option<PathCategory> {
    let relative = path.strip_prefix(agents_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    if components.is_empty() {
        return None;
    }

    let name = components[0].as_os_str().to_str()?.to_string();

    match components.len() {
        1 => Some(PathCategory::AgentDir { name }),
        2 => {
            let filename = components[1].as_os_str().to_str()?;
            if filename == RESPONSE_FILE {
                Some(PathCategory::AgentResponse { name })
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
fn categorize_under_agents(path: &Path, agents_dir: &Path) -> Option<PathCategory> {
    let relative = path.strip_prefix(agents_dir).ok()?;
    let components: Vec<_> = relative.components().collect();

    // Must be exactly one component (flat file)
    if components.len() != 1 {
        return None;
    }

    let filename = components[0].as_os_str().to_str()?;

    if let Some(id) = filename.strip_suffix(TASK_SUFFIX) {
        return Some(PathCategory::AgentTask { id: id.to_string() });
    }

    if let Some(id) = filename.strip_suffix(OUTCOME_SUFFIX) {
        return Some(PathCategory::AgentOutcome { id: id.to_string() });
    }

    None
}
```

### 3.4: Update agent handling in wiring.rs

Remove `handle_agent_dir` and `handle_agent_response`. Replace with:

```rust
PathCategory::AgentTask { id } => {
    // Daemon writes these, ignore our own writes
    trace!(id = %id, "AgentTask: ignoring (daemon wrote this)");
}
PathCategory::AgentOutcome { id } => {
    if path.exists() {
        handle_agent_outcome(&id, agents_dir, events_tx, agent_map, pending_responses);
    }
}
```

### 3.5: Update agent protocol

Agents will receive task file path and response file path from the daemon, rather than creating their own directories.

---

## Phase 4: Rename Things

### 4.1: Rename pending/ → submissions/

**File:** `crates/agent_pool/src/constants.rs`

**Before:**
```rust
pub const PENDING_DIR: &str = "pending";
```

**After:**
```rust
pub const SUBMISSIONS_DIR: &str = "submissions";
```

### 4.2: Rename variables and types throughout

- `pending_dir` → `submissions_dir`
- `ExternalTaskMap` → `SubmissionMap` (optional)
- `ExternalTaskId` → `SubmissionId` (optional)
- Update all log messages

### 4.3: Update documentation

- `SUBMISSION_PROTOCOL.md`
- `AGENT_PROTOCOL.md`

---

## Phase 5: Anonymous Worker Model

### Goal

Simplify agent protocol to a task queue. Workers are anonymous; only tasks have identity.

### Current Model (Problems)

- Agents have persistent identities (names/directories)
- Complex state machine (idle, working, kicked)
- Names carry semantic meaning

### New Model

- Workers call `get_task`, block until assigned
- Daemon returns task content + outcome file path
- Worker completes task, writes to assigned path
- Worker calls `get_task` again (back of queue)
- Heartbeats for queue starvation detection
- Names are debug-only, no uniqueness requirement

### Changes

1. Remove agent identity tracking from core state machine
2. Simplify `AgentMap` to track pending outcomes by task ID
3. `get_task` returns task + outcome path
4. Remove kicked state tracking
5. Consolidate CLI commands

---

## Task Order

1. **Phase 1** (canary sync) - Small, independent
2. **Phase 2** (flatten submissions) - **Push after this to fix CI**
3. **Phase 3** (flatten agents) - Reuses Phase 2 patterns
4. **Phase 4** (rename things) - Cleanup
5. **Phase 5** (anonymous workers) - Larger refactor, separate PR
