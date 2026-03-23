# Hook State Logging

**Blocks:** UNIFIED_ACTION_DISPATCH Phase 5 (this design must be resolved first; replaces Phase 5)

**Depends on:** UNIFIED_ACTION_DISPATCH Phases 0-4 should land first (executor trait + unified dispatch path), then this refactor uses that infrastructure.

## Motivation

Pre-hooks and post-hooks run without being recorded in the NDJSON state log. They can fail, hang forever, or produce wrong output, and there's no trace. On resume, there's no way to know whether a hook already ran for a given task.

Post-hooks block the main thread. A hanging post-hook stalls the entire event loop.

Both hooks lack timeout. Both lack independent retry. A post-hook failure after a successful action causes the action to re-run on retry.

Every side-effecting operation should be a first-class work unit: dispatched in a thread, subject to timeout, contributing to concurrency limits, recorded in the state log, and independently retryable.

## Current State

### Pre-hooks

Run inside the dispatch thread, before the action executes. Called from `dispatch_pool_task` and `dispatch_command_task` via `run_pre_hook_or_error` (`dispatch.rs:63-72`). `run_pre_hook` (`hooks.rs:69-85`) calls `run_shell_command` which blocks on `child.wait_with_output()` with no timeout. The hook receives the task's JSON value on stdin and returns a transformed value on stdout.

On failure, the dispatch function sends `SubmitResult::PreHookError(String)`. This goes through `process_retry` with `FailureKind::SubmitError`, which retries the entire task from scratch.

Not in the state log. Shares the action's `in_flight` slot.

### Post-hooks

Run on the main thread inside `process_and_finalize` (`dispatch.rs:77-110`). `run_post_hook` (`hooks.rs:90-106`) calls `run_shell_command` with the `PostHookInput` JSON on stdin. Blocks the main event loop. No timeout. Can modify the spawned tasks list.

On failure, the entire task is treated as failed and retried from scratch (action re-runs), even though the action succeeded.

Not in the state log. No `in_flight` slot (blocks main thread instead).

### Finally hooks

Run in a spawned thread via `dispatch_finally_task`. Occupy an `in_flight` slot. Recorded in state log as `StateLogEntry::FinallyRun`. Resumable. Already a first-class work unit.

### Gaps

| Hook type | Thread | In-flight slot | Timeout | State log | Resumable | Independent retry |
|-----------|--------|----------------|---------|-----------|-----------|-------------------|
| Pre-hook | Dispatch | Shared with action | No | No | No | No |
| Post-hook | Main | None (blocks loop) | No | No | No | No |
| Finally | Spawned | Yes | No | Yes | Yes | N/A |

## Resolved Design Decisions

**Hooks are separate state log entries.** Not embedded in `TaskCompleted`. Like `FinallyRun`, each hook completion is its own log entry that the apply logic processes to advance the task to the next phase.

**Post-hooks move to spawned threads.** The main thread never blocks on user code. The action's `TaskCompleted` is written before the post-hook runs. The post-hook's modifications are recorded in a separate `PostHookCompleted` entry.

**Hooks occupy their own in-flight slots.** Each phase of a task (pre-hook, action, post-hook) occupies one concurrency slot while executing.

**Hook failures retry independently.** A pre-hook failure retries the pre-hook without re-running the action. A post-hook failure retries the post-hook without re-running the action. One retry counter per task, shared across all phases. If the counter exceeds `max_retries`, the task fails.

**No no-op entries for steps without hooks.** If a step has no pre-hook, no `PreHookCompleted` is emitted. The apply logic checks the step config and skips directly to the next phase. Adding pass-through entries for consistency would create log noise without information value.

**Hooks use the step's timeout.** The same `step.options.timeout` that governs the action also governs its hooks. This keeps configuration simple. If hooks need separate timeouts in the future, that's a config addition, not an architectural change.

**On resume, missing `Completed` entries mean re-dispatch.** If the log has `TaskSubmitted` but no `PreHookCompleted` for a step with a pre-hook, the pre-hook is re-dispatched. Hooks must be idempotent (they're value-transforming shell scripts; re-running them with the same input should produce the same output).

## Task Lifecycle

A task moves through phases. Each phase is dispatched, executed in a thread, and recorded in the state log. The apply logic processes each entry and queues the next phase.

### Phase sequence for a task with pre-hook, action, and post-hook

```
TaskSubmitted              → apply queues PendingDispatch::PreHook
PreHookCompleted(Ok)       → apply stores transformed value, queues PendingDispatch::Action
TaskCompleted(Success)     → apply stores action result + children, queues PendingDispatch::PostHook
PostHookCompleted(Ok)      → apply finalizes: spawns children or removes leaf
```

### Phase sequence for a task with no hooks

```
TaskSubmitted              → apply queues PendingDispatch::Action
TaskCompleted(Success)     → apply finalizes: spawns children or removes leaf
```

### Phase sequence with pre-hook failure and retry

```
TaskSubmitted              → apply queues PendingDispatch::PreHook
PreHookCompleted(Err)      → apply increments retry, re-queues PendingDispatch::PreHook
PreHookCompleted(Ok)       → apply stores transformed value, queues PendingDispatch::Action
TaskCompleted(Success)     → apply finalizes
```

### Phase sequence with post-hook failure and retry

```
TaskSubmitted              → apply queues PendingDispatch::PreHook
PreHookCompleted(Ok)       → apply queues PendingDispatch::Action
TaskCompleted(Success)     → apply stores result, queues PendingDispatch::PostHook
PostHookCompleted(Err)     → apply increments retry, re-queues PendingDispatch::PostHook
PostHookCompleted(Ok)      → apply finalizes
```

Note: when the action succeeds but the post-hook fails, the action does NOT re-run. The retry re-dispatches only the post-hook, with the same `PostHookInput` derived from the cached action result.

## New Types

### State log entries (`barnum_state`)

```rust
/// A pre-hook completed (success or failure).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreHookCompleted {
    /// The task this pre-hook ran for.
    pub task_id: LogTaskId,
    /// The outcome of the pre-hook.
    pub outcome: HookOutcome<StepInputValue>,
}

/// A post-hook completed (success or failure).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostHookCompleted {
    /// The task this post-hook ran for.
    pub task_id: LogTaskId,
    /// The outcome of the post-hook.
    pub outcome: HookOutcome<Vec<TaskSubmitted>>,
}

/// Outcome of a hook execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum HookOutcome<T> {
    /// Hook succeeded with a value.
    Success { value: T },
    /// Hook failed.
    Failed { error: String },
}
```

The `StateLogEntry` enum gains two variants:
```rust
pub enum StateLogEntry {
    Config(StateLogConfig),
    TaskSubmitted(TaskSubmitted),
    TaskCompleted(TaskCompleted),
    FinallyRun(FinallyRun),
    PreHookCompleted(PreHookCompleted),   // NEW
    PostHookCompleted(PostHookCompleted), // NEW
}
```

### `PreHookCompleted` success value

The success value is `StepInputValue`: the transformed task value that the action will receive. This is stored so that on resume, the action can be dispatched with the pre-hook-transformed value without re-running the pre-hook.

### `PostHookCompleted` success value

The success value is `Vec<TaskSubmitted>`: the final list of children to spawn. The post-hook receives the action's raw spawned tasks and can filter, add, or transform them. The `PostHookCompleted` records the final list. On resume, these children are spawned without re-running the post-hook.

### `TaskCompleted` changes

`TaskCompleted` continues to record the action's outcome. If the step has a post-hook, `TaskCompleted` is written with the *raw* children (before post-hook modification). The post-hook modifies them, and `PostHookCompleted` records the final version.

This means `apply_completed` needs to distinguish between "leaf task done" and "task with post-hook pending." When a step has a post-hook:
- `TaskCompleted(Success)` transitions the task to `AwaitingPostHook` instead of spawning children.
- `PostHookCompleted(Success)` spawns the final children.

When a step has no post-hook, `TaskCompleted(Success)` spawns children as it does today.

## RunState Changes

### TaskState expansion

```rust
enum TaskState {
    /// Waiting for pre-hook or action dispatch.
    AwaitingPreHook(AwaitingPreHookState),
    /// Pre-hook completed, waiting for action dispatch.
    AwaitingAction(AwaitingActionState),
    /// Action completed, waiting for post-hook dispatch.
    AwaitingPostHook(AwaitingPostHookState),
    /// All phases done, waiting for children to complete.
    WaitingForChildren(WaitingState),
    /// Failed, retry pending.
    Failed,
}

struct AwaitingPreHookState {
    /// Original value (before pre-hook).
    value: StepInputValue,
    retries: u32,
}

struct AwaitingActionState {
    /// Transformed value (after pre-hook, or original if no pre-hook).
    value: StepInputValue,
    retries: u32,
}

struct AwaitingPostHookState {
    /// The raw action result needed to rebuild PostHookInput on retry.
    post_hook_input: PostHookInput,
    /// The raw children from the action (before post-hook modification).
    raw_children: Vec<Task>,
    /// The finally value for WaitingForChildren.
    finally_value: StepInputValue,
    retries: u32,
}
```

Tasks without a pre-hook skip `AwaitingPreHook` and start in `AwaitingAction`. Tasks without a post-hook skip `AwaitingPostHook` and go directly from `TaskCompleted` to `WaitingForChildren` or removal.

### PendingDispatch expansion

```rust
enum PendingDispatch {
    PreHook { task_id: LogTaskId },
    Action { task_id: LogTaskId },
    PostHook { task_id: LogTaskId },
    Finally { parent_id: LogTaskId },
}
```

### apply_entry additions

`apply_entry` gains two new match arms:

**`PreHookCompleted`:**
- Success: transition task from `AwaitingPreHook` to `AwaitingAction`, store transformed value, queue `PendingDispatch::Action`.
- Failure: if `retries < max_retries`, increment retries, re-queue `PendingDispatch::PreHook`. Otherwise, transition to `Failed` (or remove + walk up parent chain).

**`PostHookCompleted`:**
- Success: extract final children list. If children exist, transition to `WaitingForChildren` and insert children. If no children, remove task and walk up parent chain for finally detection.
- Failure: if `retries < max_retries`, increment retries, re-queue `PendingDispatch::PostHook`. Otherwise, transition to `Failed` (or remove + walk up parent chain).

**Modified `TaskSubmitted`:**
- If step has pre-hook: queue `PendingDispatch::PreHook` (instead of `PendingDispatch::Task`).
- If step has no pre-hook: queue `PendingDispatch::Action`.

**Modified `TaskCompleted`:**
- If step has post-hook: transition to `AwaitingPostHook`, queue `PendingDispatch::PostHook`. Do NOT spawn children yet.
- If step has no post-hook: spawn children / remove leaf / walk up parent chain (as today).

Note: `apply_entry` needs access to the step config to know whether hooks exist. It already receives `&Config` today.

## Engine Changes

### `flush_dispatches` expansion

`flush_dispatches` currently handles `PendingDispatch::Task` and `PendingDispatch::Finally`. It gains `PreHook`, `Action`, and `PostHook` arms. `PendingDispatch::Task` is removed (replaced by `PreHook` and `Action`).

Each arm:
1. Reads the task state to get the value/input for the phase.
2. Increments `in_flight`.
3. Spawns a thread that runs the hook/action and sends a `WorkerResult`.

### `process_worker_result` expansion

`WorkerKind` (from UNIFIED_ACTION_DISPATCH Phase 0e) expands:

```rust
pub enum WorkerKind {
    PreHook,
    Action,
    PostHook,
    Finally { parent_id: LogTaskId },
}
```

`process_worker_result` matches on `WorkerKind` and converts the raw result into the appropriate state log entry:
- `PreHook` → `StateLogEntry::PreHookCompleted`
- `Action` → `StateLogEntry::TaskCompleted` (as today)
- `PostHook` → `StateLogEntry::PostHookCompleted`
- `Finally` → `StateLogEntry::FinallyRun` (as today)

### Hooks through the executor trait

After UNIFIED_ACTION_DISPATCH Phases 0-3, all work goes through `dispatch_via_executor`. Hooks are shell scripts, so they use `ShellExecutor` (or a thin wrapper). The engine constructs the appropriate executor for each phase:

- **Pre-hook:** `ShellExecutor` with the pre-hook script. Input is the task's original value. Output is the transformed value (JSON).
- **Action:** `PoolExecutor` or `ShellExecutor` depending on step config.
- **Post-hook:** `ShellExecutor` with the post-hook script. Input is `PostHookInput` JSON. Output is the modified `PostHookInput` JSON (from which children are extracted).

All three go through `run_with_timeout` from UNIFIED_ACTION_DISPATCH Phase 4.

## Implementation Phases

### Phase A: State log types

Add `PreHookCompleted`, `PostHookCompleted`, and `HookOutcome<T>` to `barnum_state`. Add the two new variants to `StateLogEntry`. Write round-trip serialization tests. No behavioral changes.

### Phase B: RunState phase tracking

Replace `TaskState::Pending` with `AwaitingPreHook` / `AwaitingAction` / `AwaitingPostHook`. Replace `PendingDispatch::Task` with `PendingDispatch::PreHook` / `PendingDispatch::Action` / `PendingDispatch::PostHook`.

Update `apply_submitted` to check step config and queue the correct first phase. Update `apply_completed` to check for post-hook and either transition to `AwaitingPostHook` or finalize.

Add `apply_pre_hook_completed` and `apply_post_hook_completed` methods to `RunState`. Wire them into `apply_entry`.

Update existing `run_state_tests` to use the new phase names. Add tests for:
- Task with pre-hook: submitted → pre-hook queued
- Pre-hook success → action queued
- Pre-hook failure → retry pre-hook
- Pre-hook failure exhausts retries → task dropped
- Task with post-hook: action success → post-hook queued
- Post-hook success → children spawned
- Post-hook failure → retry post-hook
- Task with both hooks: full phase sequence
- Replay: PreHookCompleted removes stale PreHook dispatch
- Replay: PostHookCompleted removes stale PostHook dispatch

### Phase C: Pre-hooks as dispatched work units

Move pre-hook execution out of the dispatch thread. The engine's `flush_dispatches` handles `PendingDispatch::PreHook` by constructing a `ShellExecutor` for the pre-hook script and dispatching through `dispatch_via_executor` with `WorkerKind::PreHook`.

`process_worker_result` converts `WorkerKind::PreHook` results into `PreHookCompleted` entries. On success, the transformed value is stored in the log entry. On failure, the error string is stored.

`dispatch_via_executor` no longer calls `run_pre_hook_or_error` internally. The pre-hook is already a separate phase by this point. Remove `run_pre_hook_or_error` from `dispatch.rs`. Remove `SubmitResult::PreHookError` (pre-hook errors are now `PreHookCompleted(Failed)`).

### Phase D: Post-hooks as dispatched work units

The engine's `flush_dispatches` handles `PendingDispatch::PostHook` by constructing a `ShellExecutor` for the post-hook script and dispatching through `dispatch_via_executor` with `WorkerKind::PostHook`.

`process_worker_result` converts `WorkerKind::PostHook` results into `PostHookCompleted` entries. On success, the output is parsed as `PostHookInput` and the final children list is extracted. On failure, the error string is stored.

Remove `process_and_finalize` from `dispatch.rs`. The post-hook is no longer called from the main thread's result processing path. `process_submit_result` handles only the action result (no post-hook logic).

### Phase E: Cleanup

- Delete `PostHookInput::PreHookError` variant (pre-hook errors are handled in Phase C, they never reach the post-hook)
- Delete `run_pre_hook_or_error` and `run_pre_hook` from `hooks.rs` (replaced by executor dispatch)
- Delete `run_post_hook` from `hooks.rs` (replaced by executor dispatch)
- Delete `process_and_finalize` from `dispatch.rs` (post-hook logic moved to engine)
- Update `run_command_action` if still referenced; likely deletable

## What doesn't change

- **`run_shell_command`**: Signature and behavior unchanged. Timeout is external (from `run_with_timeout`).
- **Finally hooks**: Already first-class. No changes to `FinallyRun` or finally dispatch.
- **Config types**: `Step.pre` and `Step.post` remain `Option<HookScript>`. No new config fields.
- **`TaskCompleted` structure**: Still records success/failure with children and retry. The only change is that when a post-hook exists, children in `TaskCompleted` are the *raw* list (before post-hook modification).
