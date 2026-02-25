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
- `agent_pool::submit(&root, &payload)`
- `agent_pool::submit_file(&root, &payload)`

This bypasses the CLI parsing layer. We should use the `agent_pool submit_task` CLI command instead to test the same code path real users exercise.

### Files to update

- `greeting.rs` - 2 calls to `agent_pool::submit`
- `single_basic.rs` - `agent_pool::submit` + `submit_file`
- `single_agent_queue.rs` - `agent_pool::submit`
- `many_agents.rs` - `agent_pool::submit`

---

## Task 2: Multi-Mode Test Execution

**Goal:** Run every test in multiple submission modes to ensure all code paths are exercised.

### Background

The `submit_task` CLI has multiple options for how task data is provided:
- `--data <JSON>` - Inline task content
- `--file <PATH>` - Task content in a file

And multiple notification mechanisms:
- `--notify socket` (default) - Socket-based RPC (faster)
- `--notify file` - File-based events (works in sandboxes)

Currently, tests only exercise one combination. We should test all four:

| Mode | Data Source | Notification |
|------|-------------|--------------|
| 1    | `--data`    | `socket`     |
| 2    | `--data`    | `file`       |
| 3    | `--file`    | `socket`     |
| 4    | `--file`    | `file`       |

### Implementation

Create a test matrix that runs each test case in all four modes.

#### 2.1: Add `SubmitMode` enum

**File:** `crates/agent_pool/tests/common/mod.rs`

```rust
/// Different ways to submit tasks for testing.
#[derive(Debug, Clone, Copy)]
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

#### 2.2: Add mode-aware submit function

**File:** `crates/agent_pool/tests/common/mod.rs`

```rust
/// Submit a task using the specified mode.
pub fn submit_with_mode(root: &Path, payload: &Payload, mode: SubmitMode) -> Result<Response, Error> {
    match mode {
        SubmitMode::DataSocket => agent_pool::submit(root, payload),
        SubmitMode::DataFile => agent_pool::submit_file(root, payload),
        SubmitMode::FileSocket => {
            // Write payload to temp file, submit with --file --notify socket
            todo!()
        }
        SubmitMode::FileFile => {
            // Write payload to temp file, submit with --file --notify file
            todo!()
        }
    }
}
```

#### 2.3: Macro for test matrix

Create a macro that generates test functions for each mode:

```rust
#[macro_export]
macro_rules! test_all_modes {
    ($test_fn:ident) => {
        paste::paste! {
            #[test]
            fn [<$test_fn _data_socket>]() {
                $test_fn(SubmitMode::DataSocket);
            }

            #[test]
            fn [<$test_fn _data_file>]() {
                $test_fn(SubmitMode::DataFile);
            }

            #[test]
            fn [<$test_fn _file_socket>]() {
                $test_fn(SubmitMode::FileSocket);
            }

            #[test]
            fn [<$test_fn _file_file>]() {
                $test_fn(SubmitMode::FileFile);
            }
        }
    };
}
```

#### 2.4: Convert existing tests

Convert each test to take a `SubmitMode` parameter and use the macro:

```rust
// Before
#[test]
fn single_agent_single_task() {
    // ... setup ...
    let response = agent_pool::submit(&root, &payload);
    // ... assertions ...
}

// After
fn single_agent_single_task_impl(mode: SubmitMode) {
    // ... setup ...
    let response = submit_with_mode(&root, &payload, mode);
    // ... assertions ...
}

test_all_modes!(single_agent_single_task_impl);
```

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

### 4.1: Deterministic agent selection

Currently agent selection uses a HashSet which iterates non-deterministically. Consider using:
- BTreeMap for deterministic iteration (preferred over IndexMap)
- Or explicit ordering in tests

### 4.2: Wait for daemon ready

Ensure tests wait for daemon to be fully initialized before submitting tasks:

```rust
// Current: uses notify to wait for pending/ dir
// Better: also verify socket is listening
```

### 4.3: Proper teardown

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
