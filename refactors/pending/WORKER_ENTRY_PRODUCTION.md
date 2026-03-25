# Worker Entry Production

**Status:** Pending

**Depends on:** None

**Blocks:** APPLIER_TRAIT

## Motivation

Workers currently send raw `WorkerResult` (stdout string, exit status) on the channel. The Engine interprets results on the main thread: parsing output, validating transitions, deciding retry vs drop, allocating child/retry IDs, and constructing `StateLogEntry`. This interpretation step prevents the coordinator from being generic — it needs Engine-specific state to convert raw results into entries.

Moving interpretation to workers makes the channel carry `StateLogEntry` values directly. The coordinator receives entries and passes them to appliers without knowing how they were produced. This is the prerequisite for the Applier trait pattern (APPLIER_TRAIT.md).

## Current State

### Channel and WorkerResult (`runner/action.rs:31`)

```rust
pub struct WorkerResult {
    pub task_id: LogTaskId,
    pub task: Task,
    pub kind: WorkerKind,
    pub result: ActionResult,
}
```

Workers send `WorkerResult` on `mpsc::channel::<WorkerResult>()`. The `kind` field routes the result to either `convert_task_result` or `convert_finally_result` on the Engine.

### Result interpretation on Engine (`runner/mod.rs:509`)

`process_worker_result` receives a `WorkerResult`, calls `convert_task_result` or `convert_finally_result` depending on `kind`, applies the resulting entries to state, and flushes dispatches:

```rust
fn process_worker_result(&mut self, result: WorkerResult) -> Vec<StateLogEntry> {
    self.in_flight = self.in_flight.saturating_sub(1);
    let entries = match result.kind {
        WorkerKind::Task => self.convert_task_result(result.task_id, &result.task, result.result),
        WorkerKind::Finally { parent_id } => self.convert_finally_result(parent_id, result.result.output),
    };
    for entry in &entries { self.state.apply_entry(entry, self.config); }
    self.flush_dispatches();
    entries
}
```

### ID allocation (`runner/mod.rs:125`)

`RunState::next_id()` is a sequential `u32` counter. Only the main thread calls it, inside `convert_task_result` (for children/retries) and `convert_finally_result` (for finally children).

### Data needed for interpretation

**`convert_task_result` (`runner/mod.rs:530`)** calls `process_submit_result` which needs:
- `step: &Step` — for transition validation (`step.next`)
- `effective: &EffectiveOptions` — for retry policy
- `step_map: &HashMap<&StepName, &Step>` — for validating child step names exist

**`convert_finally_result` (`runner/mod.rs:601`)** needs:
- `state.tasks.get(&parent_id).parent_id` — grandparent ID for children's origin

All of this data is available at dispatch time.

## Target Architecture

### Channel type

```rust
type ChannelMsg = ControlFlow<io::Result<()>, StateLogEntry>;
```

Workers send `Continue(StateLogEntry::TaskCompleted(...))` or `Continue(StateLogEntry::FinallyRun(...))`. Engine sends `Break(result)` for shutdown.

### Shared ID counter

Replace `RunState::next_task_id: u32` with `Arc<AtomicU32>` owned by Engine. Workers receive a clone at dispatch time and call `fetch_add(1, Ordering::SeqCst)` to allocate IDs.

`RunState::advance_id_to(min)` stays for replay (called from `apply_entry`). It sets `self.next_task_id = max(self.next_task_id, min)` as before, but after each batch, Engine syncs the atomic counter: `id_counter.fetch_max(state.next_task_id, SeqCst)`. During live execution, the atomic counter is already ahead (workers advanced it), so `fetch_max` is a no-op. During replay, the counter is initialized from the seed batch.

### Task worker dispatch

At dispatch time, capture the data workers need for interpretation:

```rust
struct TaskWorkerContext {
    task_id: LogTaskId,
    task: Task,
    action: ShellAction,
    timeout: Option<Duration>,
    // For process_submit_result:
    step: Step,               // cloned
    effective: EffectiveOptions,
    valid_steps: HashSet<StepName>,
    // For ID allocation:
    id_counter: Arc<AtomicU32>,
    tx: Sender<ChannelMsg>,
}
```

The worker thread runs the action, calls `process_submit_result` with the captured step/options data, allocates IDs for children/retries, constructs the `StateLogEntry::TaskCompleted`, and sends `Continue(entry)` on `tx`.

`Step` is cloned at dispatch time. `valid_steps` is a `HashSet<StepName>` built from the config's step map keys. `EffectiveOptions` is computed at dispatch time (already done today). `process_submit_result` signature changes to accept `valid_steps: &HashSet<StepName>` instead of `step_map: &HashMap<&StepName, &Step>` — the only thing it checks is step name existence.

### Finally worker dispatch

At dispatch time, capture the grandparent ID:

```rust
struct FinallyWorkerContext {
    parent_id: LogTaskId,
    grandparent_id: Option<LogTaskId>,
    task: Task,
    action: ShellAction,
    timeout: Option<Duration>,
    id_counter: Arc<AtomicU32>,
    tx: Sender<ChannelMsg>,
}
```

The grandparent ID is `state.tasks.get(&parent_id).parent_id`, which is known at dispatch time. The worker runs the finally script, parses children, allocates IDs, sets children's origin to `Spawned { parent_id: grandparent_id }`, and sends `Continue(StateLogEntry::FinallyRun(...))`.

### Engine simplification

`convert_task_result` and `convert_finally_result` are deleted. `process_worker_result` becomes a simple apply-and-flush, receiving entries from the channel:

```rust
// Before: process_worker_result(WorkerResult) -> Vec<StateLogEntry>
// After: the channel already carries StateLogEntry.
// Engine just applies entries and flushes.
```

`dropped_count` tracking moves from `convert_task_result` to the apply path: when Engine sees a `TaskCompleted` with `Failed` outcome and no retry, it increments `dropped_count`.

### process_submit_result changes

The function signature changes:

```rust
// Before:
pub fn process_submit_result(
    result: ActionResult,
    task: &Task,
    step: &Step,
    options: &EffectiveOptions,
    step_map: &HashMap<&StepName, &Step>,
) -> TaskOutcome;

// After:
pub fn process_submit_result(
    result: ActionResult,
    task: &Task,
    step: &Step,
    options: &EffectiveOptions,
    valid_steps: &HashSet<StepName>,
) -> TaskOutcome;
```

`valid_steps` replaces `step_map`. The validation logic in `validate_response` / `extract_next_tasks` that checks child step existence changes from `step_map.contains_key(&name)` to `valid_steps.contains(&name)`.

### WorkerResult deletion

`WorkerResult`, `WorkerKind`, and `ActionResult` types in `action.rs` are deleted. Workers directly produce `StateLogEntry` values. `spawn_worker` takes a `TaskWorkerContext` or `FinallyWorkerContext` instead.

## Changes Summary

| Component | Before | After |
|-----------|--------|-------|
| Channel type | `mpsc::channel::<WorkerResult>()` | `mpsc::channel::<ControlFlow<io::Result<()>, StateLogEntry>>()` |
| ID allocation | `RunState::next_task_id: u32`, main thread only | `Arc<AtomicU32>`, shared with workers |
| Task interpretation | Engine main thread (`convert_task_result`) | Worker thread with captured step/options |
| Finally interpretation | Engine main thread (`convert_finally_result`) | Worker thread with captured grandparent_id |
| `process_submit_result` | Takes `&HashMap<&StepName, &Step>` | Takes `&HashSet<StepName>` |
| `WorkerResult` type | Exists | Deleted |

## Open Questions

1. **`Step` cloning cost**: `Step` contains `ActionKind` which holds script strings and paths. These are relatively small, but cloning per dispatch is new overhead. Alternatively, wrap the step map in `Arc` and share it. Probably not worth worrying about — scripts are short strings.

2. **Error logging location**: Currently `convert_finally_result` logs warnings/errors for unparseable output. With interpretation in workers, these log calls happen on worker threads. Tracing handles this fine, but worth noting the change.
