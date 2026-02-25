# Test Harness Improvements

## Overview

This document describes improvements needed to get the agent_pool tests into a robust, comprehensive state.

---

## Completed Work

The following improvements have already been made:

### 1. CLI-Based TestAgent (DONE)

**Files changed:** `crates/agent_pool/tests/common/mod.rs`

The `TestAgent` was completely rewritten to use CLI commands instead of direct file manipulation:

**Before:**
- TestAgent polled files directly from the filesystem
- Output was not captured by test framework
- Used spin loops with `thread::sleep` for synchronization

**After:**
- Uses `agent_pool get_task` for first task (registers agent)
- Uses `agent_pool next_task --data <response>` for subsequent tasks
- Spawns CLI subprocess via `Command::spawn()`
- Pipes stdout/stderr through `eprintln!()` so output respects `--nocapture`
- Uses `mpsc::sync_channel` for readiness signaling (no polling)
- Properly handles `Heartbeat` and `Kicked` control messages
- Tracks subprocess PID via `AtomicU32` for clean shutdown

### 2. Proper Task JSON Format (DONE)

**Files changed:** All test files

Updated all tests to use the proper JSON envelope format for tasks:

```rust
// Old (wrong)
Payload::inline("casual")

// New (correct)
Payload::inline(r#"{"kind":"Task","task":{"instructions":"greet","data":"casual"}}"#)
```

Also updated `integration.rs` `submit_task` helper to wrap data in the proper envelope:
```rust
let task_envelope = serde_json::json!({
    "kind": "Task",
    "task": { "instructions": "test task", "data": data_value }
});
let payload = serde_json::json!({
    "kind": "Inline",
    "content": task_envelope.to_string()
});
```

### 3. Daemon Output Capture (DONE)

**Files changed:** `crates/agent_pool/tests/common/mod.rs`

`AgentPoolHandle` now pipes daemon stdout/stderr through `eprintln!()`:

```rust
if let Some(stdout) = process.stdout.take() {
    output_threads.push(thread::spawn(move || {
        let reader = BufReader::new(stdout);
        for line in reader.lines().map_while(Result::ok) {
            eprintln!("[daemon stdout] {line}");
        }
    }));
}
```

This ensures daemon output is captured by the test framework and visible with `--nocapture`.

### 4. Removed Incompatible Tests (DONE)

Removed tests that relied on internal file-based protocol details that no longer apply with CLI-based agents:

- `file_protocol_basic` in `single_basic.rs`
- `multiple_agents_direct_dispatch` in `many_agents.rs`
- `sequential_tasks_same_agent` in `single_agent_queue.rs`

### 5. JSON Assertion Fixes (DONE)

Fixed assertions that assumed specific JSON formatting:

- Changed exact string equality to `contains()` checks (field ordering varies)
- Removed space expectations after colons (`"id":"A"` not `"id": "A"`)

### 6. CI Timeouts (DONE)

**Files changed:** `.github/workflows/ci.yml`

Added 5-minute timeout to all CI jobs to prevent hanging builds.

### 7. Clippy/Fmt Fixes (DONE)

**Files changed:** `crates/agent_pool/tests/common/mod.rs`

- Fixed import ordering
- Used `map_while(Result::ok)` instead of `flatten()` for line iteration
- Added backticks to doc comments for code references
- Added allow attributes for test-specific clippy lints

---

## Current State

The tests now use CLI-based `TestAgent` that interacts with the daemon via `get_task` and `next_task` CLI commands. This is good because:
- Tests exercise the same code paths as real agents
- Output is properly captured via `eprintln!()` respecting `--nocapture`
- Uses proper synchronization (channels) instead of polling

However, there are several areas that need improvement.

---

## Task 1: Use CLI for All Task Submission

**Goal:** Replace library function calls with CLI commands to test the full stack.

### Problem

Tests currently use library functions directly:
- `agent_pool::submit(&root, &payload)` - socket-based RPC
- `agent_pool::submit_file(&root, &payload)` - file-based polling

This bypasses the CLI parsing layer. We should use the `agent_pool submit_task` CLI command instead.

### Understanding the Code

**Daemon readiness signal:** The daemon creates `pending/` directory AFTER the watcher is running (see `wiring.rs:183-184`):
```rust
// Create pending_dir AFTER watcher is running.
// Clients use pending_dir existence as the "ready" signal.
```

**Test harness already uses notify** to wait for this directory (`common/mod.rs:388-425`):
```rust
fn wait_for_directory_creation<F, T>(watch_root: &Path, target_dir: &Path, action: F) -> T
```

**Library submit functions:**
- `submit()` connects to Unix socket, sends payload, waits for response
- `submit_file()` polls with `thread::sleep` (violates coding patterns, but we're replacing it anyway)

### Implementation

#### 1.1: Add `submit_via_cli` helper function

**File:** `crates/agent_pool/tests/common/mod.rs`

Add this function that spawns the CLI and returns the parsed response:

```rust
use agent_pool::Response;
use std::process::Command;

/// Submit a task via the CLI.
///
/// Executes: `agent_pool submit_task --pool <root> --data <payload_json> --notify <method>`
pub fn submit_via_cli(root: &Path, payload_json: &str, notify: &str) -> io::Result<Response> {
    let bin = find_agent_pool_binary();

    let output = Command::new(&bin)
        .arg("submit_task")
        .arg("--pool")
        .arg(root)
        .arg("--data")
        .arg(payload_json)
        .arg("--notify")
        .arg(notify)
        .output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("CLI failed: {stderr}"),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
```

#### 1.2: Update `greeting.rs`

**Before:**
```rust
use agent_pool::{Payload, Response};

let casual = agent_pool::submit(
    &root,
    &Payload::inline(r#"{"kind":"Task","task":{"instructions":"greet","data":"casual"}}"#),
)
.expect("Submit failed");
```

**After:**
```rust
use common::submit_via_cli;
use agent_pool::Response;

let casual = submit_via_cli(
    &root,
    r#"{"kind":"Task","task":{"instructions":"greet","data":"casual"}}"#,
    "socket",
)
.expect("Submit failed");
```

Note: Remove `Payload` import since we pass the JSON string directly.

#### 1.3: Update `single_basic.rs`

**single_agent_single_task - Before:**
```rust
let response = agent_pool::submit(
    &root,
    &Payload::inline(r#"{"kind":"Task","task":{"instructions":"echo","data":"Hello, World!"}}"#),
)
.expect("Submit failed");
```

**After:**
```rust
let response = submit_via_cli(
    &root,
    r#"{"kind":"Task","task":{"instructions":"echo","data":"Hello, World!"}}"#,
    "socket",
)
.expect("Submit failed");
```

**file_based_submit - Before:**
```rust
let response = submit_file(
    &root,
    &Payload::inline(r#"{"kind":"Task","task":{"instructions":"echo","data":"Hello via file!"}}"#),
)
.expect("File submit failed");
```

**After:**
```rust
let response = submit_via_cli(
    &root,
    r#"{"kind":"Task","task":{"instructions":"echo","data":"Hello via file!"}}"#,
    "file",
)
.expect("File submit failed");
```

#### 1.4: Update `single_agent_queue.rs`

**Before:**
```rust
let task_json =
    format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":"{task}"}}}}"#);
thread::spawn(move || {
    agent_pool::submit(&root, &Payload::inline(task_json)).expect("Submit failed")
})
```

**After:**
```rust
let task_json =
    format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":"{task}"}}}}"#);
thread::spawn(move || {
    submit_via_cli(&root, &task_json, "socket").expect("Submit failed")
})
```

#### 1.5: Update `many_agents.rs`

**Before:**
```rust
let task = format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":"Task-{i}"}}}}"#);
thread::spawn(move || {
    agent_pool::submit(&root, &Payload::inline(task)).expect("Submit failed")
})
```

**After:**
```rust
let task = format!(r#"{{"kind":"Task","task":{{"instructions":"echo","data":"Task-{i}"}}}}"#);
thread::spawn(move || {
    submit_via_cli(&root, &task, "socket").expect("Submit failed")
})
```

#### 1.6: Remove unused imports

After conversion, remove from each file:
- `use agent_pool::Payload;` (no longer needed)
- `use agent_pool::submit_file;` (if present)

Add instead:
- `use common::submit_via_cli;`

---

## Task 2: Multi-Mode Test Execution

**Goal:** Run every test in all four submission modes to ensure complete coverage.

### The Four Modes

| Mode | CLI Args | Description |
|------|----------|-------------|
| `DataSocket` | `--data <json> --notify socket` | Inline JSON, socket RPC |
| `DataFile` | `--data <json> --notify file` | Inline JSON, file polling |
| `FileSocket` | `--file <path> --notify socket` | JSON from file, socket RPC |
| `FileFile` | `--file <path> --notify file` | JSON from file, file polling |

### Implementation

#### 2.1: Add `SubmitMode` enum

**File:** `crates/agent_pool/tests/common/mod.rs`

```rust
/// Submission mode for testing different CLI code paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SubmitMode {
    /// --data with --notify socket
    DataSocket,
    /// --data with --notify file
    DataFile,
    /// --file with --notify socket
    FileSocket,
    /// --file with --notify file
    FileFile,
}

impl SubmitMode {
    /// All submit modes for matrix testing.
    pub const ALL: [SubmitMode; 4] = [
        SubmitMode::DataSocket,
        SubmitMode::DataFile,
        SubmitMode::FileSocket,
        SubmitMode::FileFile,
    ];
}
```

#### 2.2: Add `submit_with_mode` function (uses CLI)

**File:** `crates/agent_pool/tests/common/mod.rs`

This replaces the simpler `submit_via_cli` from Task 1, adding support for all 4 modes:

```rust
use std::io::{self, Write};
use tempfile::NamedTempFile;

/// Submit a task using the specified mode (all modes use CLI).
pub fn submit_with_mode(root: &Path, payload_json: &str, mode: SubmitMode) -> io::Result<Response> {
    let bin = find_agent_pool_binary();
    let mut cmd = Command::new(&bin);

    cmd.arg("submit_task")
        .arg("--pool")
        .arg(root);

    // Configure data source and notification method based on mode
    match mode {
        SubmitMode::DataSocket => {
            cmd.arg("--data").arg(payload_json);
            cmd.arg("--notify").arg("socket");
        }
        SubmitMode::DataFile => {
            cmd.arg("--data").arg(payload_json);
            cmd.arg("--notify").arg("file");
        }
        SubmitMode::FileSocket => {
            let mut temp = NamedTempFile::new()?;
            temp.write_all(payload_json.as_bytes())?;
            temp.flush()?;
            cmd.arg("--file").arg(temp.path());
            cmd.arg("--notify").arg("socket");
            // Note: temp file is kept alive until command completes
        }
        SubmitMode::FileFile => {
            let mut temp = NamedTempFile::new()?;
            temp.write_all(payload_json.as_bytes())?;
            temp.flush()?;
            cmd.arg("--file").arg(temp.path());
            cmd.arg("--notify").arg("file");
        }
    };

    let output = cmd.output()?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(io::Error::new(
            io::ErrorKind::Other,
            format!("CLI failed (mode={mode:?}): {stderr}"),
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    serde_json::from_str(&stdout)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}
```

**Dependencies to add to `Cargo.toml`:**
```toml
[dev-dependencies]
tempfile = "3"
```

#### 2.3: Use `rstest` for parameterized tests

The `rstest` crate is Rust's equivalent of Jest's `describe.each` - it generates separate test functions for each parameter combination.

**Dependencies to add to `Cargo.toml`:**
```toml
[dev-dependencies]
tempfile = "3"
rstest = "0.18"
```

#### 2.4: Convert `greeting.rs` to multi-mode

**Before (single test):**
```rust
#[test]
fn greeting_casual_and_formal() {
    let root = setup_test_dir(TEST_DIR);
    if !is_ipc_available(&root) { ... }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::greeting(&root, "friendly-bot", Duration::from_millis(10));
    agent.wait_ready();

    let casual = agent_pool::submit(&root, &Payload::inline(...)).expect("Submit failed");
    // ... assertions ...

    cleanup_test_dir(TEST_DIR);
}
```

**After (4 tests via rstest):**
```rust
use rstest::rstest;
use common::SubmitMode;

#[rstest]
#[case(SubmitMode::DataSocket)]
#[case(SubmitMode::DataFile)]
#[case(SubmitMode::FileSocket)]
#[case(SubmitMode::FileFile)]
fn greeting_casual_and_formal(#[case] mode: SubmitMode) {
    // Use mode in test dir name to avoid conflicts when tests run in parallel
    let test_dir = format!("{TEST_DIR}_{mode:?}");
    let root = setup_test_dir(&test_dir);

    if !is_ipc_available(&root) {
        eprintln!("SKIP: IPC not available");
        cleanup_test_dir(&test_dir);
        return;
    }

    let _pool = AgentPoolHandle::start(&root);
    let mut agent = TestAgent::greeting(&root, "friendly-bot", Duration::from_millis(10));
    agent.wait_ready();

    let casual = submit_with_mode(
        &root,
        r#"{"kind":"Task","task":{"instructions":"greet","data":"casual"}}"#,
        mode,
    )
    .expect("Submit failed");

    let Response::Processed { stdout, .. } = casual else {
        panic!("Expected Processed response");
    };
    assert_eq!(stdout.trim(), "Hi friendly-bot, how are ya?");

    // ... formal test ...

    let _ = agent.stop();
    cleanup_test_dir(&test_dir);
}
```

This generates 4 test functions automatically:
- `greeting_casual_and_formal::case_1` (DataSocket)
- `greeting_casual_and_formal::case_2` (DataFile)
- `greeting_casual_and_formal::case_3` (FileSocket)
- `greeting_casual_and_formal::case_4` (FileFile)

#### 2.5: Convert other test files

Apply the same pattern to:
- `single_basic.rs` - `single_agent_single_task`, `file_based_submit`
- `single_agent_queue.rs` - `single_agent_queues_multiple_tasks`
- `many_agents.rs` - `multiple_agents_parallel_tasks`

**Key changes for each:**
1. Add `use rstest::rstest;` and `use common::SubmitMode;`
2. Add `#[rstest]` and `#[case(...)]` attributes
3. Add `mode: SubmitMode` parameter
4. Include `mode:?` in test directory name
5. Replace `agent_pool::submit(...)` with `submit_with_mode(&root, json, mode)`

---

## Task 3: CLI Command Naming

**Goal:** Clean up confusing `get_task` vs `register` CLI commands.

### Current State

The CLI has two commands that do the same thing:
- `get_task` - "Wait for and return the next task (for agents)"
- `register` - "Register as an agent and wait for first task (alias for get_task)"

### Options

1. **Rename `get_task` to `register`** and deprecate `get_task` (add hidden alias for backwards compat)
2. **Keep both** but clarify docs that `register` is preferred for first call
3. **Different behavior** - `register` only registers, `get_task` waits (breaking change)

### Recommendation

Option 1: Rename to `register` since that's what it actually does. The command:
1. Creates the agent directory
2. Waits for daemon to acknowledge (heartbeat)
3. Returns first task

The name "register" is more accurate than "get_task".

---

## Task 4: Test Output Improvements

**Goal:** Make test output clearer and more useful.

### 3.1: Structured logs

Replace ad-hoc `eprintln!("[agent X] message")` with structured tracing:

```rust
use tracing::{info, debug};

info!(agent = %agent_id, "received task");
debug!(agent = %agent_id, task = %task_json, "task content");
```

### 3.2: Test timing

Add timing information to understand test performance:

```rust
let start = Instant::now();
// ... test code ...
info!(elapsed = ?start.elapsed(), "test completed");
```

---

## Task 5: Test Reliability

**Goal:** Eliminate flaky tests and race conditions.

Note: Tests should pass regardless of which agent is assigned a task. We don't need deterministic agent selection.

### 5.1: Wait for daemon ready

Ensure tests wait for daemon to be fully initialized before submitting tasks:

```rust
// Current: uses notify to wait for pending/ dir
// Better: also verify socket is listening
```

### 5.2: Proper teardown

Ensure all tests clean up properly:
- Stop all agents
- Wait for daemon to process stop
- Clean up test directories

---

## Task 6: Test Coverage

**Goal:** Ensure all important scenarios are tested.

### Missing tests:

1. **Agent timeout** - Agent doesn't respond within timeout
2. **Agent crash** - Agent process dies mid-task
3. **Daemon restart** - Agent reconnects after daemon restart
4. **Large payloads** - Tasks with large data
5. **Concurrent submit** - Multiple clients submitting simultaneously
6. **Heartbeat failure** - Agent fails to respond to heartbeat
7. **Task cancellation** - Client withdraws task before completion

### Priority order:

1. Agent timeout (affects production reliability)
2. Agent crash (error recovery)
3. Heartbeat failure (liveness detection)
4. Large payloads (edge case)
5. Concurrent submit (load handling)

---

## Implementation Order

1. **Task 1: Use CLI for All Submission** (highest priority - test full stack)
2. **Task 2: Multi-Mode Testing** (second - ensures all paths tested)
3. **Task 5: Test Reliability** (third - reduces flakiness)
4. **Task 6: Test Coverage** (fourth - expands coverage)
5. **Task 3: CLI Naming** (fifth - improves UX)
6. **Task 4: Output Improvements** (sixth - improves debugging)

---

## Notes

- Each task should be implementable independently
- Tests should continue passing as changes are made
- Document any breaking changes to test infrastructure
