# Hook State Logging

**Blocks:** UNIFIED_ACTION_DISPATCH (Phase 5 depends on this being resolved first)

## Motivation

Pre-hooks and post-hooks run without being recorded in the NDJSON state log. They can fail, hang forever, or produce wrong output, and there's no trace. On resume, there's no way to know whether a hook already ran for a given task. The state log should record every side-effecting operation so that the run is fully observable and resumable.

## Current State

### Pre-hooks

**Where they run:** Inside the dispatch thread, before the action executes. Called from `dispatch_pool_task` and `dispatch_command_task` via `run_pre_hook_or_error` (`dispatch.rs:63-72`).

**Execution model:** `run_pre_hook` (`hooks.rs:69-85`) calls `run_shell_command`, which spawns `sh -c <script>` with the task's JSON value on stdin. It blocks on `child.wait_with_output()` with no timeout. The hook's stdout is parsed as JSON and becomes the new task value.

**On failure:** The dispatch function sends `SubmitResult::PreHookError(String)` back on the channel. `process_submit_result` (`response.rs:100-109`) routes this to `process_retry` with `FailureKind::SubmitError`. The post-hook receives `PostHookInput::PreHookError`.

**What's not logged:** The pre-hook execution itself. The state log sees only `TaskSubmitted` (before hook) and `TaskCompleted` (after everything, including hook failure). There's no way to distinguish "pre-hook failed" from "action failed" in the state log. On resume, there's no way to know whether the pre-hook already ran and transformed the value.

**Concurrency:** Pre-hooks share the dispatch thread's `in_flight` slot with the action. They run sequentially: pre-hook blocks, then action runs. From the engine's perspective, one slot covers both.

### Post-hooks

**Where they run:** On the main thread, inside `process_and_finalize` (`dispatch.rs:77-110`). Called after `process_submit_result` returns and only if `step.post` is `Some`.

**Execution model:** `run_post_hook` (`hooks.rs:90-106`) calls `run_shell_command` with the `PostHookInput` JSON on stdin. It blocks the main thread on `child.wait_with_output()` with no timeout. The hook's stdout is parsed as `PostHookInput` and can modify the `next` tasks array.

**On failure:** `process_and_finalize` (`dispatch.rs:102-105`) calls `process_retry` with `FailureKind::SubmitError`. The entire task is treated as failed, even though the action itself succeeded.

**What's not logged:** The post-hook execution. `TaskCompleted` is written after `process_and_finalize` returns. A post-hook failure appears as a task failure in the log, with no indication that the action succeeded but the post-hook failed. On resume, a task whose post-hook failed will be re-dispatched from scratch (action re-runs).

**Concurrency:** Post-hooks run on the main thread. They don't occupy an `in_flight` slot — they block the entire event loop. While a post-hook runs, no worker results are processed, no new tasks are dispatched.

### Finally hooks

**Where they run:** In a spawned thread via `dispatch_finally_task` (`dispatch.rs:200-216`).

**Execution model:** Calls `run_shell_command` directly (no pre-hook). Occupies an `in_flight` slot. The result is sent on the channel and processed as `SubmitResult::Finally`.

**What IS logged:** `StateLogEntry::FinallyRun` records the finally hook's execution and any children it spawned. The state log knows whether a finally ran and what it produced.

**Why finally is different:** Finally hooks were designed with resumability from the start. The `FinallyRun` entry is the boundary between "children completed" and "parent done". Without it, resume can't determine whether the finally needs to re-run.

### Summary of gaps

| Hook type | Thread | In-flight slot | Timeout | State log entry | Resumable |
|-----------|--------|----------------|---------|-----------------|-----------|
| Pre-hook | Dispatch thread | Shared with action | None | No | No |
| Post-hook | Main thread | None (blocks event loop) | None | No | No |
| Finally | Spawned thread | Yes | None | `FinallyRun` | Yes |

## Problems

### 1. Main-thread blocking by post-hooks

Post-hooks run on the main thread inside `process_and_finalize`. A hanging post-hook blocks the entire event loop: no worker results are processed, no new tasks are dispatched, the whole run stalls. This is the most acute operational risk.

### 2. No observability for hook execution

Hook runs leave no trace in the state log. When debugging a failed run, there's no way to determine whether a hook ran, what it received, or what it returned. The only signal is the task's final outcome, which conflates hook failures with action failures.

### 3. Resume re-runs actions unnecessarily

If a pre-hook fails, the task is marked failed in the state log (as `TaskCompleted` with failure). On resume, the retry re-runs the entire task from scratch, which is correct. But if a post-hook fails after a successful action, the task is also marked failed. On resume, the action re-runs even though it already succeeded. For expensive actions (agent pool submissions), this wastes significant resources.

### 4. No timeout for hooks

Both `run_pre_hook` and `run_post_hook` call `run_shell_command`, which blocks on `wait_with_output()` indefinitely. A hook that hangs (waiting on a network resource, deadlocked, infinite loop) stalls either a dispatch thread (pre-hook) or the entire event loop (post-hook).

## Design Questions

These are the decisions that need to be made before implementation. Capturing them here for discussion.

### Q1: Should post-hooks move off the main thread?

Post-hooks currently run on the main thread because they need to modify the task outcome before it's written to the state log. Moving them to a spawned thread means the action's `TaskCompleted` entry must be written before the post-hook runs, and the post-hook's modifications become a separate log entry.

Options:
- **Keep on main thread, add timeout.** Simpler. The main thread blocks for at most `timeout` seconds. Risk: even a bounded block delays all other processing.
- **Move to spawned thread, add state log entry.** The action writes `TaskCompleted` with the raw spawned tasks. The post-hook runs in a thread, writes a `PostHookCompleted` entry that modifies the spawned tasks. More complex, but the main thread never blocks on user code.

### Q2: What state log entries do we need?

Minimal set:
```
PreHookStarted  { task_id }
PreHookCompleted { task_id, outcome: Result<Value, String> }
PostHookStarted  { task_id }
PostHookCompleted { task_id, outcome: Result<PostHookInput, String> }
```

Or a more compact approach where hooks are embedded in the existing `TaskCompleted` entry (like how children are embedded in `TaskSuccess`). The tradeoff is resumability granularity vs. log complexity.

### Q3: Should hooks occupy their own in-flight slots?

Currently pre-hooks share the action's slot. If hooks get their own slots, the effective concurrency for actions drops (each task occupies 1-3 slots: pre + action + post). If hooks stay in the action's slot, the concurrency model is simpler but hooks aren't independently cancellable.

### Q4: Should hook failures be retried independently of the action?

Currently, a pre-hook failure retries the entire task (pre-hook + action). A post-hook failure also retries the entire task (action re-runs). If hooks are independent work units, a post-hook failure could retry just the post-hook, preserving the action's result.

### Q5: What happens on resume with partial hook execution?

If the state log has `PreHookStarted` but no `PreHookCompleted`, did the hook run to completion (crash before logging) or not? The safe answer is to re-run it, which means pre-hooks must be idempotent. This is already implicitly required (they're shell scripts that transform values), but it should be documented.

## Next Steps

This document is intentionally research-focused. The design decisions above need to be resolved before writing an implementation plan. The answers will determine whether this is a small change (add timeout + log entries, keep current threading model) or a significant architectural shift (move post-hooks off main thread, independent retry).
