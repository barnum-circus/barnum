# Test Harness Improvements

## Overview

This document describes improvements needed to get the agent_pool tests into a robust, comprehensive state.

## Current State

The tests now use CLI-based `TestAgent` that interacts with the daemon via `get_task` and `next_task` CLI commands. This is good because:
- Tests exercise the same code paths as real agents
- Output is properly captured via `eprintln!()` respecting `--nocapture`
- Uses proper synchronization (channels) instead of polling

However, there are several areas that need improvement.

---

## Task 1: Multi-Mode Test Execution

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

#### 1.1: Add `SubmitMode` enum

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

#### 1.2: Add mode-aware submit function

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

#### 1.3: Macro for test matrix

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

#### 1.4: Convert existing tests

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

## Task 2: CLI Command Naming

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

## Task 3: Test Output Improvements

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

## Task 4: Test Reliability

**Goal:** Eliminate flaky tests and race conditions.

### 4.1: Deterministic agent selection

Currently agent selection uses a HashSet which iterates non-deterministically. Consider using:
- IndexMap for deterministic iteration
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

## Task 5: Test Coverage

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

1. **Task 1: Multi-Mode Testing** (highest priority - ensures all paths tested)
2. **Task 4: Test Reliability** (second - reduces flakiness)
3. **Task 5: Test Coverage** (third - expands coverage)
4. **Task 2: CLI Naming** (fourth - improves UX)
5. **Task 3: Output Improvements** (fifth - improves debugging)

---

## Notes

- Each task should be implementable independently
- Tests should continue passing as changes are made
- Document any breaking changes to test infrastructure
