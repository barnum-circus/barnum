# Test Harness Improvements

## Overview

This document describes improvements needed to get the agent_pool tests into a robust, comprehensive state.

---

## Completed Work

### 1. CLI-Based TestAgent (DONE)

The `TestAgent` uses CLI commands (`get_task`, `next_task`) instead of direct file manipulation. Uses `mpsc::sync_channel` for readiness signaling (no polling).

### 2. Proper Task JSON Format (DONE)

All tests use the proper JSON envelope: `{"kind":"Task","task":{"instructions":"...","data":...}}`

### 3. Daemon Output Capture (DONE)

`AgentPoolHandle` pipes daemon stdout/stderr through `eprintln!()` for `--nocapture` support.

### 4. CI Timeouts (DONE)

5-minute timeout on all CI jobs. Test job now downloads CLI binary from `build-linux-x64`.

### 5. Clippy/Fmt Fixes (DONE)

All lint issues resolved.

### 6. CLI-Based Task Submission (DONE)

**Files changed:** `common/mod.rs`, `greeting.rs`, `single_basic.rs`, `single_agent_queue.rs`, `many_agents.rs`

Tests now use `submit_via_cli()` and `submit_with_mode()` instead of `agent_pool::submit()` library function. This tests the full CLI stack.

### 7. Multi-Mode Testing with rstest (DONE)

**Files changed:** All test files except `integration.rs`

Tests use `#[rstest]` with `#[case(SubmitMode::*)]` to run all 4 submission modes:
- `DataSocket` - `--data` with `--notify socket`
- `DataFile` - `--data` with `--notify file`
- `FileSocket` - `--file` with `--notify socket`
- `FileFile` - `--file` with `--notify file`

---

## Remaining Tasks

### Task 1: CLI Command Naming

**Goal:** Clean up confusing CLI command names.

#### 1.1: Rename `get_task` to `register`

The `get_task` command actually registers the agent AND waits for the first task. Rename to `register` since that's what it does.

- Remove `get_task` command entirely
- Keep `register` as the canonical name
- Update `TestAgent` to use `register` instead of `get_task`
- Update docs and examples

#### 1.2: Consider `complete_task` for final response

Currently `deregister` just removes the agent. Consider whether we need a `complete_task` command that:
- Submits the final task response (`--data`)
- Deregisters the agent

This would make the agent lifecycle clearer:
```
register -> (next_task --data <response>)* -> complete_task --data <final_response>
```

**Open question:** Should `deregister` accept `--data` for the final response, or should there be a separate `complete_task` command?

---

### Task 2: Test Output Improvements

**Goal:** Make test output clearer and more useful.

#### 2.1: Structured logging with tracing

Replace ad-hoc `eprintln!("[agent X] message")` with structured tracing:

```rust
use tracing::{info, debug};

info!(agent = %agent_id, "received task");
debug!(agent = %agent_id, task = %task_json, "task content");
```

#### 2.2: Test timing

Add timing information:

```rust
let start = Instant::now();
// ... test code ...
info!(elapsed = ?start.elapsed(), "test completed");
```

#### 2.3: Tracing subscriber setup

Add a test helper that initializes tracing with env filter:

```rust
pub fn init_test_tracing() {
    let _ = tracing_subscriber::fmt()
        .with_env_filter("agent_pool=debug")
        .with_test_writer()
        .try_init();
}
```

---

### Task 3: Proper Teardown

**Goal:** Ensure all tests clean up properly.

#### 3.1: Stop all agents before cleanup

Ensure tests call `agent.stop()` for all agents before `cleanup_test_dir()`.

#### 3.2: Wait for daemon to process stops

After stopping agents, give daemon time to process the deregistrations before killing it.

#### 3.3: Clean up on panic

Use `Drop` or `scopeguard` to ensure cleanup happens even if test panics.

---

### Task 4: Test Coverage Matrix

**Goal:** Ensure all important scenarios are tested.

#### Test Dimensions

```
                           ┌─────────────────────────────────────────────────────────────┐
                           │                    SUBMISSION METHOD                         │
                           ├─────────────┬─────────────┬─────────────┬─────────────────┬──┤
                           │ DataSocket  │ DataFile    │ FileSocket  │ FileFile        │  │
┌──────────────────────────┼─────────────┼─────────────┼─────────────┼─────────────────┤  │
│ AGENTS                   │             │             │             │                 │  │
├──────────────────────────┼─────────────┼─────────────┼─────────────┼─────────────────┤  │
│ 1 agent, 1 task          │ ✓ rstest    │ ✓ rstest    │ ✓ rstest    │ ✓ rstest        │  │
│ 1 agent, N tasks (queue) │ ✓ rstest    │ ✓ rstest    │ ✓ rstest    │ ✓ rstest        │  │
│ N agents, N tasks        │ ✓ rstest    │ ✓ rstest    │ ✓ rstest    │ ✓ rstest        │  │
│ Agent joins mid-process  │ ✓ integr    │             │             │                 │  │
│ Agent deregisters        │ ✓ integr    │             │             │                 │  │
│ Tasks queued before agent│ ✓ integr    │             │             │                 │  │
└──────────────────────────┴─────────────┴─────────────┴─────────────┴─────────────────┴──┘
```

#### Scenario Categories

**A. Happy Path (covered by rstest multi-mode)**
- Single agent, single task
- Single agent, multiple tasks (queuing)
- Multiple agents, parallel tasks
- Greeting agent (custom processor)

**B. Agent Lifecycle (integration.rs)**
- Agent registration
- Agent deregistration
- Agent joins while tasks processing
- Tasks queued before any agent registers

**C. Error Recovery (TODO)**
- Agent timeout (doesn't respond)
- Agent crash (process dies mid-task)
- Agent fails to respond to heartbeat
- Large payload handling

**D. Edge Cases (TODO)**
- Rapid burst of submissions
- Identical task content
- Response isolation (correct response to correct submitter)

#### Missing Tests

| Scenario | Priority | Notes |
|----------|----------|-------|
| Agent timeout | High | Agent assigned task but doesn't respond within timeout |
| Agent crash | High | Agent process dies mid-task |
| Heartbeat failure | Medium | Agent fails to respond to heartbeat |
| Task cancellation | Medium | Client withdraws task before completion |
| Daemon restart | Low | Agent reconnects after daemon restart |
| Large payloads | Low | Tasks with very large data |

#### Test DAG

```
                    ┌────────────────────┐
                    │   Daemon Starts    │
                    └─────────┬──────────┘
                              │
              ┌───────────────┼───────────────┐
              ▼               ▼               ▼
      ┌───────────┐   ┌───────────┐   ┌───────────────┐
      │ 0 agents  │   │ 1 agent   │   │ N agents      │
      │ N tasks   │   │ M tasks   │   │ M tasks       │
      │ (queued)  │   │           │   │               │
      └─────┬─────┘   └─────┬─────┘   └───────┬───────┘
            │               │                 │
            │               ▼                 ▼
            │       ┌───────────────┐ ┌───────────────┐
            │       │ Agent timeout │ │ Agent crash   │
            │       └───────┬───────┘ └───────┬───────┘
            │               │                 │
            │               ▼                 ▼
            │       ┌───────────────┐ ┌───────────────┐
            │       │ Task retried  │ │ Task failed   │
            │       │ or failed     │ │ returned      │
            │       └───────────────┘ └───────────────┘
            │
            ▼
    ┌───────────────┐
    │ Agent joins   │
    │ picks up task │
    └───────────────┘
```

---

## Implementation Order

1. **Task 3: Proper Teardown** - Ensures reliable test runs
2. **Task 4: Test Coverage** - Add missing error recovery tests
3. **Task 2: Test Output** - Better debugging
4. **Task 1: CLI Naming** - UX improvement (can be done anytime)

---

## Notes

- `integration.rs` intentionally tests the raw file protocol (direct writes to `pending/`), not CLI submission. Keep it separate.
- Tests should pass regardless of which agent is assigned a task - no deterministic agent selection needed.
- Each test file uses its own subdirectory in `.test-data/` for parallel execution.
