# Test Harness Improvements

## Status

**Tests pass** with `--test-threads=1`. Multi-threaded test execution times out due to CLI spawn overhead (tracked in `todos.md`).

**Completed:**
- CLI-based TestAgent
- CLI-based task submission
- Multi-mode testing with rstest (6 submission modes)
- Raw file submission mode
- Two-enum refactor (DataSource × NotifyMethod)

**Remaining:**
- Task 1: Raw File Agent mode
- Task 4: Replace polling with notify in file transport
- Task 6: Missing test scenarios (timeouts, crashes, etc.)

## Overview

This document describes improvements needed to get the troupe tests into a robust, comprehensive state.

---

## Test Inventory

### Current Tests

| File | Test | What It Tests | Submission Modes | Agent Mode |
|------|------|---------------|------------------|------------|
| `greeting.rs` | `greeting_casual_and_formal` | Custom processor (greeting agent with casual/formal) | rstest × 6 | CLI |
| `single_basic.rs` | `single_agent_single_task` | Basic single agent, single task | rstest × 6 | CLI |
| `single_agent_queue.rs` | `single_agent_queues_multiple_tasks` | Single agent processes 4 tasks (queuing) | rstest × 6 | CLI |
| `many_agents.rs` | `multiple_agents_parallel_tasks` | 3 agents process 6 tasks in parallel | rstest × 6 | CLI |
| `integration.rs` | `basic_submit` | Basic submit/response flow | rstest × 6 | CLI |
| `integration.rs` | `single_agent_multiple_tasks` | Sequential tasks to single agent | rstest × 6 | CLI |
| `integration.rs` | `multiple_agents_parallel` | 2 agents process 4 tasks in parallel | rstest × 6 | CLI |
| `integration.rs` | `agent_deregistration` | Agent stops, new agent picks up work | rstest × 6 | CLI |
| `integration.rs` | `tasks_queued_before_agents` | Tasks submitted before agent registers | rstest × 6 | CLI |
| `integration.rs` | `rapid_task_burst` | 10 tasks submitted rapidly | rstest × 6 | CLI |
| `integration.rs` | `identical_task_content` | 5 tasks with identical content | rstest × 6 | CLI |
| `integration.rs` | `agent_joins_mid_processing` | Second agent joins while first is processing | rstest × 6 | CLI |
| `integration.rs` | `response_isolation` | Responses go to correct submitters | rstest × 6 | CLI |

### Submission Modes (rstest × 6)

Tests use rstest with a cross-product of two enums:

**DataSource** (2 options):
- `Inline` - Content passed inline (`--data` or `{"kind": "Inline", "content": ...}`)
- `FileReference` - Content in separate file (`--file` or `{"kind": "FileReference", "path": ...}`)

**NotifyMethod** (3 options):
- `Socket` - CLI with `--notify socket`
- `File` - CLI with `--notify file`
- `Raw` - Direct write to `pending/`, wait with notify

This gives 2 × 3 = 6 combinations per test.

### Agent Modes

Currently all agents use the CLI (`register`, `next_task`).

**Missing:** Raw file protocol (direct writes to `agents/` directory)

---

## Mode Matrix

### Submission Modes (DataSource × NotifyMethod = 6)

| DataSource | NotifyMethod | CLI Flag / Protocol | Description |
|------------|--------------|---------------------|-------------|
| `Inline` | `Socket` | `--data --notify socket` | Inline JSON, socket notification |
| `Inline` | `File` | `--data --notify file` | Inline JSON, file notification |
| `Inline` | `Raw` | Direct write (Inline envelope) | Write `{"kind":"Inline","content":...}` to `pending/<task_id>/task.json` |
| `FileReference` | `Socket` | `--file --notify socket` | JSON from file, socket notification |
| `FileReference` | `File` | `--file --notify file` | JSON from file, file notification |
| `FileReference` | `Raw` | Direct write (FileReference envelope) | Write `{"kind":"FileReference","path":...}` to `pending/<task_id>/task.json` |

### Agent Modes (2 total)

| Mode | Description |
|------|-------------|
| `CLI` | Use `register`/`next_task` CLI commands |
| `RawFile` | Write directly to `agents/<name>/response.json` |

### Coverage Goal

Run each test scenario with:
- 6 submission modes (done)
- 2 agent modes (TODO)

= 12 combinations per test

---

## Completed Work

### 1. CLI-Based TestAgent (DONE)
TestAgent uses CLI commands (`register`, `next_task`).

### 2. CLI-Based Task Submission (DONE)
All tests use `submit_with_mode()` instead of library functions.

### 3. Multi-Mode Testing with rstest (DONE)
All tests use `#[rstest]` with 6 submission modes (DataSource × NotifyMethod).

### 4. CLI Rename (DONE)
`get_task` renamed to `register`.

### 5. Raw File Submission Mode (DONE)
Added `NotifyMethod::Raw` that writes directly to `pending/<task_id>/task.json` and waits for response using notify.

### 6. Two-Enum Refactor (DONE)
Split single `SubmitMode` enum into `DataSource` × `NotifyMethod` for clearer semantics.

```rust
pub enum DataSource {
    Inline,        // --data or {"kind": "Inline", ...}
    FileReference, // --file or {"kind": "FileReference", ...}
}

pub enum NotifyMethod {
    Socket, // --notify socket
    File,   // --notify file
    Raw,    // direct write to pending/
}
```

---

## Remaining Tasks

### Task 1: Add Raw File Protocol to Agent Modes

Add `AgentMode` enum to test agents using raw file protocol:

```rust
pub enum AgentMode {
    CLI,
    RawFile,
}

impl TestAgent {
    pub fn with_mode(mode: AgentMode, ...) -> Self {
        match mode {
            AgentMode::CLI => Self::start(...),
            AgentMode::RawFile => Self::start_raw_file(...),
        }
    }
}
```

### Task 2: CLI Command Improvements

#### 2.1: Consider `complete_task` for final response

Current lifecycle:
```
register -> (next_task --data <response>)* -> deregister
```

Consider adding `complete_task`:
```
register -> (next_task --data <response>)* -> complete_task --data <final_response>
```

Or add `--data` flag to `deregister`:
```
register -> (next_task --data <response>)* -> deregister --data <final_response>
```

**Open question:** What's the cleanest API?

### Task 3: Test Output Improvements

#### 3.1: Structured logging with tracing

Replace `eprintln!()` with structured tracing:
```rust
use tracing::{info, debug};
info!(agent = %agent_id, "received task");
```

#### 3.2: Tracing subscriber setup

Add test helper:
```rust
pub fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("troupe=debug")
        .with_test_writer()
        .try_init();
}
```

### Task 4: Replace Polling with Notify in File Transport

**File:** `crates/troupe/src/client/submit_file.rs`

The file-based submission currently polls every 100ms waiting for `response.json`. This is inefficient and adds latency.

Replace with proper file watching using `notify`:
1. Set up watcher on `pending/<uuid>/` directory
2. Write `task.json`
3. Block until `response.json` is created
4. Clean up

This will make file-based submission as responsive as socket-based.

### Task 5: Proper Teardown

Ensure tests clean up properly even on panic. Use `scopeguard` or similar.

### Task 6: Missing Test Scenarios

| Scenario | Priority | Notes |
|----------|----------|-------|
| Agent timeout | High | Agent assigned task but doesn't respond |
| Agent crash | High | Agent process dies mid-task |
| Heartbeat failure | Medium | Agent fails to respond to heartbeat |
| Task cancellation | Medium | Client withdraws task before completion |
| Large payloads | Low | Tasks with very large data |

### Task 7: Move Tests to Separate Crate

Move integration tests into their own crate (e.g., `troupe_tests`) so they can only use the public API. This ensures:
- Tests exercise the same interface that external users would use
- No accidental coupling to internal implementation details
- Clear separation between unit tests (in `troupe`) and integration tests

---

## Implementation Order

1. **Task 1: Raw File Agent** - Complete the mode matrix
2. **Task 4: Replace Polling with Notify** - Performance
3. **Task 5: Proper Teardown** - Reliability
4. **Task 6: Missing Scenarios** - Coverage
5. **Task 3: Test Output** - Debugging
6. **Task 2: CLI Improvements** - UX
7. **Task 7: Separate Test Crate** - API hygiene

---

## Notes

- Each test file uses its own subdirectory in `.test-data/` for parallel execution
- Tests should pass regardless of which agent is assigned a task
- `integration.rs` was converted from raw file protocol to CLI - now tests same things as other files
