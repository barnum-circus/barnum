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

Run inside the dispatch thread, before the action executes. Called from `dispatch_pool_task` and `dispatch_command_task` via `run_pre_hook_or_error` (`dispatch.rs:63-72`):

```rust
// dispatch.rs:63-72
fn run_pre_hook_or_error(
    pre_hook: Option<&HookScript>,
    original_value: &StepInputValue,
    working_dir: &Path,
) -> Result<StepInputValue, String> {
    let Some(hook) = pre_hook else {
        return Ok(original_value.clone());
    };
    run_pre_hook(hook, &original_value.0, working_dir).map(StepInputValue)
}
```

`run_pre_hook` (`hooks.rs:69-85`) calls `run_shell_command` which blocks on `child.wait_with_output()` with no timeout. The hook receives the task's JSON value on stdin and returns a transformed value on stdout.

On failure, the dispatch function sends `SubmitResult::PreHookError(String)`. This goes through `process_retry` with `FailureKind::SubmitError`, which retries the entire task from scratch.

Not in the state log. Shares the action's `in_flight` slot.

### Post-hooks

Run on the main thread inside `process_and_finalize` (`dispatch.rs:77-110`):

```rust
// dispatch.rs:77-110
pub(super) fn process_and_finalize(
    result: SubmitResult,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
    working_dir: &Path,
) -> TaskOutcome {
    let ProcessedSubmit { outcome, post_input } =
        process_submit_result(result, task, step, schemas);

    if let Some(hook) = &step.post {
        match run_post_hook(hook, &post_input, working_dir) {
            Ok(modified) => match outcome {
                TaskOutcome::Success(TaskSuccess { finally_value, .. }) => {
                    let tasks = extract_next_tasks(&modified);
                    TaskOutcome::Success(TaskSuccess {
                        spawned: tasks,
                        finally_value,
                    })
                }
                other => other,
            },
            Err(e) => {
                warn!(step = %task.step, error = %e, "post hook failed");
                process_retry(task, &step.options, FailureKind::SubmitError)
            }
        }
    } else {
        outcome
    }
}
```

Blocks the main event loop. No timeout. Can modify the spawned tasks list. On failure, the entire task is treated as failed and retried from scratch (action re-runs), even though the action succeeded.

Not in the state log. No `in_flight` slot.

### Finally hooks

Run in a spawned thread via `dispatch_finally_task`. Occupy an `in_flight` slot. Recorded in state log as `StateLogEntry::FinallyRun`. Resumable. Already a first-class work unit.

### How `dispatch_task` currently hands off pre-hooks

`dispatch_task` (`mod.rs:718-760`) clones the pre-hook and passes it into the dispatch thread. The pre-hook runs synchronously before the action in the same thread:

```rust
// mod.rs:722-740 (Pool branch)
Action::Pool(..) => {
    let pre_hook = step.pre.clone();
    let docs = generate_step_docs(step, self.config);
    let timeout = step.options.timeout;
    let pool = self.pool.clone();

    thread::spawn(move || {
        dispatch_pool_task(
            task_id, task, pre_hook.as_ref(),
            &docs, timeout, &pool, &tx,
        );
    });
}
```

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

**No no-op entries for steps without hooks.** If a step has no pre-hook, no `PreHookCompleted` is emitted. The apply logic checks the step config and skips directly to the next phase.

**Hooks use the step's timeout.** The same `step.options.timeout` that governs the action also governs its hooks.

**On resume, missing `Completed` entries mean re-dispatch.** If the log has `TaskSubmitted` but no `PreHookCompleted` for a step with a pre-hook, the pre-hook is re-dispatched. Hooks must be idempotent.

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

When the action succeeds but the post-hook fails, the action does NOT re-run. The retry re-dispatches only the post-hook, with the same `PostHookInput` derived from the cached action result.

## Proposed Changes

### Phase A: State log types

**File: `crates/barnum_state/src/types.rs`**

Add new types and `StateLogEntry` variants.

Before (`types.rs:15-26`):
```rust
pub enum StateLogEntry {
    Config(StateLogConfig),
    TaskSubmitted(TaskSubmitted),
    TaskCompleted(TaskCompleted),
    FinallyRun(FinallyRun),
}
```

After:
```rust
pub enum StateLogEntry {
    Config(StateLogConfig),
    TaskSubmitted(TaskSubmitted),
    PreHookCompleted(PreHookCompleted),
    TaskCompleted(TaskCompleted),
    PostHookCompleted(PostHookCompleted),
    FinallyRun(FinallyRun),
}

/// A pre-hook completed (success or failure).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreHookCompleted {
    pub task_id: LogTaskId,
    pub outcome: HookOutcome<StepInputValue>,
}

/// A post-hook completed (success or failure).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PostHookCompleted {
    pub task_id: LogTaskId,
    pub outcome: HookOutcome<Vec<TaskSubmitted>>,
}

/// Outcome of a hook execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum HookOutcome<T> {
    Success { value: T },
    Failed { error: String },
}
```

`PreHookCompleted` success value is `StepInputValue`: the transformed task value that the action receives. Stored so that on resume, the action dispatches with the pre-hook-transformed value without re-running the pre-hook.

`PostHookCompleted` success value is `Vec<TaskSubmitted>`: the final children list after post-hook modification. The post-hook receives the action's raw output (as `PostHookInput` JSON) and can filter/add/transform children.

No behavioral changes. Add round-trip serialization tests.

---

### Phase B: RunState phase tracking

**File: `crates/barnum_config/src/runner/mod.rs`**

#### B1: Replace `TaskState` and `PendingDispatch`

Before (`mod.rs:77-112`):
```rust
enum TaskState {
    Pending(PendingState),
    WaitingForChildren(WaitingState),
    Failed,
}

struct PendingState {
    value: StepInputValue,
}

struct WaitingState {
    pending_children_count: NonZeroU16,
    finally_value: StepInputValue,
}

struct PendingTask {
    task_id: LogTaskId,
}

struct PendingFinally {
    parent_id: LogTaskId,
}

enum PendingDispatch {
    Task(PendingTask),
    Finally(PendingFinally),
}
```

After:
```rust
enum TaskState {
    AwaitingPreHook(AwaitingPreHookState),
    AwaitingAction(AwaitingActionState),
    AwaitingPostHook(AwaitingPostHookState),
    WaitingForChildren(WaitingState),
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
    /// The PostHookInput to feed the post-hook (rebuilt on retry from cached fields).
    post_hook_input: PostHookInput,
    /// The raw children from the action (before post-hook modification).
    raw_children: Vec<Task>,
    /// The finally value for WaitingForChildren.
    finally_value: StepInputValue,
    retries: u32,
}

struct WaitingState {
    pending_children_count: NonZeroU16,
    finally_value: StepInputValue,
}

enum PendingDispatch {
    PreHook { task_id: LogTaskId },
    Action { task_id: LogTaskId },
    PostHook { task_id: LogTaskId },
    Finally { parent_id: LogTaskId },
}
```

Delete `PendingTask`, `PendingFinally`, `PendingState`.

#### B2: Modify `apply_submitted` to queue correct first phase

Before (`mod.rs:186-194`):
```rust
let prev = self.tasks.insert(
    submitted.task_id,
    TaskEntry {
        step: submitted.step.clone(),
        parent_id,
        state: TaskState::Pending(PendingState {
            value: submitted.value.clone(),
        }),
    },
);
```

After:
```rust
// apply_submitted now takes &Config to check whether step has a pre-hook.
// The initial state depends on whether the step has a pre-hook.
let has_pre_hook = config
    .step_map()
    .get(&submitted.step)
    .is_some_and(|s| s.pre.is_some());

let state = if has_pre_hook {
    TaskState::AwaitingPreHook(AwaitingPreHookState {
        value: submitted.value.clone(),
        retries: 0,
    })
} else {
    TaskState::AwaitingAction(AwaitingActionState {
        value: submitted.value.clone(),
        retries: 0,
    })
};

let prev = self.tasks.insert(
    submitted.task_id,
    TaskEntry {
        step: submitted.step.clone(),
        parent_id,
        state,
    },
);
```

`apply_submitted` signature changes to take `&Config`.

#### B3: Modify `apply_entry` for `TaskSubmitted` to queue correct dispatch

Before (`mod.rs:288-293`):
```rust
StateLogEntry::TaskSubmitted(s) => {
    self.advance_id_to(s.task_id.0 + 1);
    self.apply_submitted(s);
    self.pending_dispatches
        .push_back(PendingDispatch::Task(PendingTask { task_id: s.task_id }));
}
```

After:
```rust
StateLogEntry::TaskSubmitted(s) => {
    self.advance_id_to(s.task_id.0 + 1);
    self.apply_submitted(s, config);
    let has_pre_hook = config
        .step_map()
        .get(&s.step)
        .is_some_and(|step| step.pre.is_some());
    if has_pre_hook {
        self.pending_dispatches
            .push_back(PendingDispatch::PreHook { task_id: s.task_id });
    } else {
        self.pending_dispatches
            .push_back(PendingDispatch::Action { task_id: s.task_id });
    }
}
```

#### B4: Add `apply_entry` arm for `PreHookCompleted`

New arm in `apply_entry`:
```rust
StateLogEntry::PreHookCompleted(phc) => {
    // Remove stale dispatch (replay: completed before dispatched)
    self.pending_dispatches.retain(
        |d| !matches!(d, PendingDispatch::PreHook { task_id } if *task_id == phc.task_id),
    );
    self.apply_pre_hook_completed(phc, config);
}
```

New method on `RunState`:
```rust
fn apply_pre_hook_completed(
    &mut self,
    phc: &barnum_state::PreHookCompleted,
    config: &Config,
) {
    let entry = self.tasks.get_mut(&phc.task_id)
        .expect("[P080] pre-hook task must exist");
    let TaskState::AwaitingPreHook(state) = &mut entry.state else {
        panic!("[P081] pre-hook completed for task not in AwaitingPreHook state");
    };

    match &phc.outcome {
        HookOutcome::Success { value } => {
            entry.state = TaskState::AwaitingAction(AwaitingActionState {
                value: value.clone(),
                retries: state.retries,
            });
            self.pending_dispatches
                .push_back(PendingDispatch::Action { task_id: phc.task_id });
        }
        HookOutcome::Failed { .. } => {
            let max_retries = config.step_map()
                .get(&entry.step)
                .map_or(0, |s| s.options.max_retries);
            state.retries += 1;
            if state.retries <= max_retries {
                self.pending_dispatches
                    .push_back(PendingDispatch::PreHook { task_id: phc.task_id });
            } else {
                // Exhausted retries — remove task, walk up parent chain
                let removed = self.tasks.remove(&phc.task_id)
                    .expect("[P082] task must exist for removal");
                // Return parent_id for finally detection (handled by apply_entry caller)
                // ... same pattern as permanent failure in apply_completed
            }
        }
    }
}
```

**Complication:** `apply_pre_hook_completed` failure with exhausted retries needs to walk up the parent chain for finally detection, same as `apply_completed` does for permanent failures. The return-parent-id-and-walk pattern is reused. Extract a helper `remove_and_get_parent` to share between `apply_completed` and `apply_pre_hook_completed`.

#### B5: Modify `apply_completed` to handle post-hook phase

Before (`mod.rs:217-231`, the success-with-children arm):
```rust
barnum_state::TaskOutcome::Success(success) if !success.children.is_empty() => {
    entry.state = TaskState::WaitingForChildren(WaitingState {
        pending_children_count: NonZeroU16::new(
            success.children.len().try_into().unwrap(),
        ).unwrap(),
        finally_value: success.finally_value.clone(),
    });
    for child in &success.children {
        self.apply_submitted(child);
    }
    None
}
```

After:
```rust
barnum_state::TaskOutcome::Success(success) => {
    let has_post_hook = config.step_map()
        .get(&entry.step)
        .is_some_and(|s| s.post.is_some());

    if has_post_hook {
        // Don't spawn children yet — post-hook runs first and may modify them.
        let retries = match &entry.state {
            TaskState::AwaitingAction(s) => s.retries,
            _ => panic!("[P083] completed task not in AwaitingAction state"),
        };
        entry.state = TaskState::AwaitingPostHook(AwaitingPostHookState {
            post_hook_input: /* build PostHookInput from success data */,
            raw_children: /* raw Task list before post-hook */,
            finally_value: success.finally_value.clone(),
            retries,
        });
        // Queue post-hook dispatch (done in apply_entry, not here)
        None
    } else if !success.children.is_empty() {
        // No post-hook, has children → WaitingForChildren as before
        entry.state = TaskState::WaitingForChildren(WaitingState {
            pending_children_count: NonZeroU16::new(
                success.children.len().try_into().unwrap(),
            ).unwrap(),
            finally_value: success.finally_value.clone(),
        });
        for child in &success.children {
            self.apply_submitted(child, config);
        }
        None
    } else {
        // No post-hook, leaf success → remove
        let removed = self.tasks.remove(&completed.task_id)
            .expect("[P033] task must exist for removal");
        removed.parent_id
    }
}
```

`apply_completed` signature changes to take `&Config`.

The `apply_entry` arm for `TaskCompleted` also needs to queue `PendingDispatch::PostHook` when the step has a post-hook:
```rust
StateLogEntry::TaskCompleted(c) => {
    // ... existing stale-dispatch removal and id tracking ...

    let parent_id = self.apply_completed(c, config);

    let has_post_hook = config.step_map()
        .get(/* step name for c.task_id */)
        .is_some_and(|s| s.post.is_some());

    if has_post_hook {
        // apply_completed transitioned to AwaitingPostHook, queue the dispatch
        self.pending_dispatches
            .push_back(PendingDispatch::PostHook { task_id: c.task_id });
    } else {
        // Queue children/retry dispatches as before
        match &c.outcome {
            barnum_state::TaskOutcome::Success(s) => {
                for child in &s.children {
                    self.pending_dispatches
                        .push_back(PendingDispatch::Action { task_id: child.task_id });
                    // or PreHook if child's step has pre-hook
                }
            }
            // ... retry dispatch ...
        }
        // ... finally walk as before ...
    }
}
```

**Complication:** Queuing child dispatches now needs to check each child's step for pre-hooks (to decide `PendingDispatch::PreHook` vs `PendingDispatch::Action`). This duplicates logic from `apply_submitted`. The cleaner approach: `apply_submitted` already sets the task's initial state correctly, and `apply_entry`'s `TaskSubmitted` arm already queues the right dispatch. Since `apply_completed` calls `apply_submitted` for children, and those children also go through the `TaskSubmitted` arm... actually, no — children embedded in `TaskCompleted` are inserted via `apply_submitted` inside `apply_completed`, but their dispatches are queued by the `apply_entry` `TaskCompleted` arm. This coupling is already present in the current code. The same pattern works: `apply_completed` inserts children, `apply_entry` queues their dispatches based on step config.

#### B6: Add `apply_entry` arm for `PostHookCompleted`

```rust
StateLogEntry::PostHookCompleted(phc) => {
    // Remove stale dispatch (replay)
    self.pending_dispatches.retain(
        |d| !matches!(d, PendingDispatch::PostHook { task_id } if *task_id == phc.task_id),
    );
    let parent_id = self.apply_post_hook_completed(phc, config);

    // If finalized (leaf or exhausted retries), walk up for finally
    if let Some(pid) = parent_id
        && let Some(finally_id) = self.walk_up_for_finally(pid, config)
    {
        self.pending_dispatches
            .push_back(PendingDispatch::Finally { parent_id: finally_id });
    }
}
```

New method on `RunState`:
```rust
fn apply_post_hook_completed(
    &mut self,
    phc: &barnum_state::PostHookCompleted,
    config: &Config,
) -> Option<LogTaskId> {
    let entry = self.tasks.get_mut(&phc.task_id)
        .expect("[P084] post-hook task must exist");
    let TaskState::AwaitingPostHook(state) = &mut entry.state else {
        panic!("[P085] post-hook completed for task not in AwaitingPostHook state");
    };

    match &phc.outcome {
        HookOutcome::Success { value: children } => {
            if children.is_empty() {
                // Leaf after post-hook — remove task
                let removed = self.tasks.remove(&phc.task_id)
                    .expect("[P086] task must exist for removal");
                removed.parent_id
            } else {
                // Spawn final children
                let finally_value = state.finally_value.clone();
                entry.state = TaskState::WaitingForChildren(WaitingState {
                    pending_children_count: NonZeroU16::new(
                        children.len().try_into().unwrap(),
                    ).unwrap(),
                    finally_value,
                });
                for child in children {
                    self.apply_submitted(child, config);
                    // dispatch queued by caller based on child step config
                }
                None
            }
        }
        HookOutcome::Failed { .. } => {
            let max_retries = config.step_map()
                .get(&entry.step)
                .map_or(0, |s| s.options.max_retries);
            state.retries += 1;
            if state.retries <= max_retries {
                self.pending_dispatches
                    .push_back(PendingDispatch::PostHook { task_id: phc.task_id });
                None
            } else {
                let removed = self.tasks.remove(&phc.task_id)
                    .expect("[P087] task must exist for removal");
                removed.parent_id
            }
        }
    }
}
```

#### B7: Update `FinallyRun` arm to use new dispatch variant names

Before (`mod.rs:358-363`):
```rust
for child in &f.children {
    self.pending_dispatches
        .push_back(PendingDispatch::Task(PendingTask {
            task_id: child.task_id,
        }));
}
```

After:
```rust
for child in &f.children {
    let has_pre_hook = config.step_map()
        .get(&child.step)
        .is_some_and(|s| s.pre.is_some());
    if has_pre_hook {
        self.pending_dispatches
            .push_back(PendingDispatch::PreHook { task_id: child.task_id });
    } else {
        self.pending_dispatches
            .push_back(PendingDispatch::Action { task_id: child.task_id });
    }
}
```

Same change applies everywhere `PendingDispatch::Task` was queued for children.

**Extract helper:** This "queue first dispatch for task based on step config" logic appears in `TaskSubmitted`, `TaskCompleted` (for children), and `FinallyRun` (for children). Extract:
```rust
fn queue_first_dispatch(&mut self, task_id: LogTaskId, step: &StepName, config: &Config) {
    let has_pre_hook = config.step_map()
        .get(step)
        .is_some_and(|s| s.pre.is_some());
    if has_pre_hook {
        self.pending_dispatches.push_back(PendingDispatch::PreHook { task_id });
    } else {
        self.pending_dispatches.push_back(PendingDispatch::Action { task_id });
    }
}
```

#### B8: Tests

Update all existing `run_state_tests`. The test helpers `step()` and `step_with_finally()` currently create steps with no hooks (`pre: None, post: None`). Add `step_with_pre()`, `step_with_post()`, `step_with_both()` helpers.

Tests to update:
- Every assertion on `TaskState::Pending` changes to `TaskState::AwaitingAction` (steps without hooks skip pre-hook)
- Every assertion on `PendingDispatch::Task` changes to `PendingDispatch::Action`
- Every `has_task_dispatch` helper changes to `has_action_dispatch`

Tests to add:
- Task with pre-hook: submitted → `PendingDispatch::PreHook` queued, state is `AwaitingPreHook`
- Pre-hook success → state is `AwaitingAction`, `PendingDispatch::Action` queued
- Pre-hook failure, retries remaining → stays `AwaitingPreHook`, `PendingDispatch::PreHook` re-queued
- Pre-hook failure, retries exhausted → task removed, parent walk for finally
- Task with post-hook: action success → state is `AwaitingPostHook`, `PendingDispatch::PostHook` queued
- Post-hook success with children → state is `WaitingForChildren`, children inserted
- Post-hook success with no children → task removed, parent walk for finally
- Post-hook failure, retries remaining → stays `AwaitingPostHook`, `PendingDispatch::PostHook` re-queued
- Post-hook failure, retries exhausted → task removed, parent walk for finally
- Full lifecycle: pre-hook → action → post-hook → children → finally
- Replay: `PreHookCompleted` removes stale `PreHook` dispatch
- Replay: `PostHookCompleted` removes stale `PostHook` dispatch

---

### Phase C: Pre-hooks as dispatched work units

**File: `crates/barnum_config/src/runner/mod.rs`**

#### C1: Expand `flush_dispatches` with `PreHook` arm

Before (`mod.rs:676-694`, the `Task` arm):
```rust
PendingDispatch::Task(PendingTask { task_id }) => {
    let entry = self.state.tasks.get_mut(&task_id)
        .expect("[P064] pending task not in map");
    let TaskState::Pending(pending) = &mut entry.state else {
        panic!("[P065] pending task not in Pending state");
    };
    let value = std::mem::replace(
        &mut pending.value,
        StepInputValue(serde_json::Value::Null),
    );
    let step_name = entry.step.clone();
    let task = Task::new(step_name.as_str(), value);

    self.in_flight += 1;
    self.dispatch_task(task_id, task);
}
```

After (replaces `Task` arm, adds `PreHook` arm before `Action` arm):
```rust
PendingDispatch::PreHook { task_id } => {
    let entry = self.state.tasks.get(&task_id)
        .expect("[P088] pre-hook task not in map");
    let TaskState::AwaitingPreHook(state) = &entry.state else {
        panic!("[P089] pre-hook task not in AwaitingPreHook state");
    };
    let step = self.step_map.get(&entry.step)
        .expect("[P015] unknown step");
    let script = step.pre.clone()
        .expect("[P090] pre-hook dispatch for step without pre-hook");
    let value = state.value.clone();
    let working_dir = self.pool.working_dir.clone();
    let tx = self.tx.clone();

    let executor = Box::new(ShellExecutor {
        script: script.to_string(),
        step_name: entry.step.clone(),
        working_dir: self.pool.working_dir.clone(),
    });
    let timeout = step.options.timeout.map(Duration::from_secs);

    self.in_flight += 1;
    thread::spawn(move || {
        dispatch_via_executor(
            task_id, Task::new(/* step */, value),
            WorkerKind::PreHook,
            None, // no pre-hook for the pre-hook itself
            executor, timeout, &working_dir, &tx,
        );
    });
}
PendingDispatch::Action { task_id } => {
    let entry = self.state.tasks.get_mut(&task_id)
        .expect("[P064] action task not in map");
    let TaskState::AwaitingAction(state) = &mut entry.state else {
        panic!("[P065] action task not in AwaitingAction state");
    };
    let value = std::mem::replace(
        &mut state.value,
        StepInputValue(serde_json::Value::Null),
    );
    let step_name = entry.step.clone();
    let task = Task::new(step_name.as_str(), value);

    self.in_flight += 1;
    self.dispatch_task(task_id, task);
}
```

#### C2: Expand `process_worker_result` with `PreHook` arm

Before (`mod.rs:514-535`):
```rust
fn process_worker_result(&mut self, result: WorkerResult) -> Vec<StateLogEntry> {
    let WorkerResult { task_id, task, result: submit_result } = result;
    self.in_flight = self.in_flight.saturating_sub(1);
    let entries = match submit_result {
        dispatch::SubmitResult::Finally(dispatch::FinallyResult { value, output }) => {
            self.convert_finally_result(task_id, value, output)
        }
        other => self.convert_task_result(task_id, &task, other),
    };
    // ...
}
```

After (assuming UNIFIED_ACTION_DISPATCH Phase 0e's `WorkerKind` is in place):
```rust
fn process_worker_result(&mut self, result: WorkerResult) -> Vec<StateLogEntry> {
    self.in_flight = self.in_flight.saturating_sub(1);
    let entries = match result.kind {
        WorkerKind::PreHook => {
            let output = match result.result {
                SubmitResult::Action(ActionResult { output, .. }) => output,
                SubmitResult::PreHookError(e) => Err(e),
            };
            let outcome = match output {
                Ok(stdout) => match serde_json::from_str(&stdout) {
                    Ok(value) => HookOutcome::Success { value: StepInputValue(value) },
                    Err(e) => HookOutcome::Failed { error: format!("invalid JSON: {e}") },
                },
                Err(e) => HookOutcome::Failed { error: e },
            };
            vec![StateLogEntry::PreHookCompleted(PreHookCompleted {
                task_id: result.task_id,
                outcome,
            })]
        }
        WorkerKind::Action => self.convert_task_result(result.task_id, &result.task, result.result),
        WorkerKind::PostHook => { /* Phase D */ }
        WorkerKind::Finally { parent_id } => { /* existing */ }
    };
    for entry in &entries {
        self.state.apply_entry(entry, self.config);
    }
    self.flush_dispatches();
    entries
}
```

#### C3: Remove pre-hook from `dispatch_via_executor`

After UNIFIED_ACTION_DISPATCH Phases 0-3, `dispatch_via_executor` has a `pre_hook` parameter. Since pre-hooks are now a separate phase, `dispatch_via_executor` no longer runs pre-hooks. Remove the `pre_hook` parameter and `run_pre_hook_or_error` call.

Before (from UNIFIED_ACTION_DISPATCH Phase 1c):
```rust
pub fn dispatch_via_executor(
    task_id: LogTaskId,
    task: Task,
    kind: WorkerKind,
    pre_hook: Option<&HookScript>,
    executor: Box<dyn Executor>,
    timeout: Option<Duration>,
    working_dir: &Path,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let value = match run_pre_hook_or_error(pre_hook, &task.value, working_dir) {
        Ok(v) => v,
        Err(e) => {
            let _ = tx.send(WorkerResult { task_id, task, kind, result: SubmitResult::PreHookError(e) });
            return;
        }
    };
    let output = run_with_timeout(executor, &value.0, timeout);
    let _ = tx.send(WorkerResult { task_id, task, kind, result: SubmitResult::Action(ActionResult { value, output }) });
}
```

After:
```rust
pub fn dispatch_via_executor(
    task_id: LogTaskId,
    task: Task,
    kind: WorkerKind,
    executor: Box<dyn Executor>,
    timeout: Option<Duration>,
    tx: &mpsc::Sender<WorkerResult>,
) {
    let output = run_with_timeout(executor, &task.value.0, timeout);
    let _ = tx.send(WorkerResult {
        task_id, task, kind,
        result: SubmitResult::Action(ActionResult {
            value: task.value.clone(),
            output,
        }),
    });
}
```

Delete `run_pre_hook_or_error`. Delete `SubmitResult::PreHookError`. Delete `run_pre_hook` from `hooks.rs`.

#### C4: Update `dispatch_task` to not pass pre-hook

All `dispatch_task` call sites (both Pool and Command branches) remove the `pre_hook` parameter.

---

### Phase D: Post-hooks as dispatched work units

**File: `crates/barnum_config/src/runner/mod.rs`**

#### D1: Add `PostHook` arm in `flush_dispatches`

```rust
PendingDispatch::PostHook { task_id } => {
    let entry = self.state.tasks.get(&task_id)
        .expect("[P091] post-hook task not in map");
    let TaskState::AwaitingPostHook(state) = &entry.state else {
        panic!("[P092] post-hook task not in AwaitingPostHook state");
    };
    let step = self.step_map.get(&entry.step)
        .expect("[P015] unknown step");
    let script = step.post.clone()
        .expect("[P093] post-hook dispatch for step without post-hook");
    let input_json = serde_json::to_string(&state.post_hook_input)
        .unwrap_or_default();
    let working_dir = self.pool.working_dir.clone();
    let tx = self.tx.clone();

    let executor = Box::new(ShellExecutor {
        script: script.to_string(),
        step_name: entry.step.clone(),
        working_dir: self.pool.working_dir.clone(),
    });
    let timeout = step.options.timeout.map(Duration::from_secs);

    self.in_flight += 1;
    thread::spawn(move || {
        // The task value is the PostHookInput JSON, not the original value
        let task = Task::new(/* step */, StepInputValue(serde_json::json!(input_json)));
        dispatch_via_executor(
            task_id, task, WorkerKind::PostHook,
            executor, timeout, &tx,
        );
    });
}
```

**Complication:** The `ShellExecutor` receives `&serde_json::Value` in `execute()`, but the post-hook's input is a `PostHookInput` JSON string, not the task's value. Two options:
- Pass the serialized `PostHookInput` as the executor's input value (the executor doesn't care what the JSON represents).
- Create a `PostHookExecutor` that captures the `PostHookInput` and serializes it internally.

The first option is simpler: the `ShellExecutor` just runs `run_shell_command(script, &serde_json::to_string(value), working_dir)`. The value happens to be a `PostHookInput` serialized as JSON. The executor doesn't know or care.

#### D2: Add `PostHook` arm in `process_worker_result`

```rust
WorkerKind::PostHook => {
    let output = match result.result {
        SubmitResult::Action(ActionResult { output, .. }) => output,
    };
    let outcome = match output {
        Ok(stdout) => match serde_json::from_str::<PostHookInput>(&stdout) {
            Ok(modified) => {
                let tasks = extract_next_tasks(&modified);
                let children: Vec<TaskSubmitted> = tasks
                    .into_iter()
                    .map(|child| {
                        let id = self.state.next_id();
                        TaskSubmitted {
                            task_id: id,
                            step: child.step,
                            value: child.value,
                            origin: TaskOrigin::Spawned(SpawnedOrigin {
                                parent_id: Some(result.task_id),
                            }),
                        }
                    })
                    .collect();
                HookOutcome::Success { value: children }
            }
            Err(e) => HookOutcome::Failed { error: format!("invalid JSON: {e}") },
        },
        Err(e) => HookOutcome::Failed { error: e },
    };
    vec![StateLogEntry::PostHookCompleted(PostHookCompleted {
        task_id: result.task_id,
        outcome,
    })]
}
```

#### D3: Delete `process_and_finalize`

Before (`dispatch.rs:77-110`):
```rust
pub(super) fn process_and_finalize(
    result: SubmitResult,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
    working_dir: &Path,
) -> TaskOutcome {
    // ...
    if let Some(hook) = &step.post {
        match run_post_hook(hook, &post_input, working_dir) { ... }
    } else {
        outcome
    }
}
```

After: deleted entirely. `convert_task_result` calls `process_submit_result` directly (no post-hook wrapping). The post-hook is a separate phase handled by `PendingDispatch::PostHook` → `flush_dispatches` → `process_worker_result` → `PostHookCompleted`.

#### D4: Delete post-hook code from `hooks.rs`

Delete `run_post_hook` (`hooks.rs:90-106`). Delete `extract_next_tasks` from `dispatch.rs` (moved into `process_worker_result`'s PostHook arm). Delete `PostHookInput::PreHookError` variant (pre-hook errors never reach the post-hook now).

---

### Phase E: Cleanup

- Delete `run_pre_hook_or_error` from `dispatch.rs`
- Delete `run_pre_hook` from `hooks.rs`
- Delete `run_post_hook` from `hooks.rs`
- Delete `run_command_action` from `hooks.rs` (if still present after UNIFIED_ACTION_DISPATCH)
- Delete `process_and_finalize` from `dispatch.rs`
- Delete `SubmitResult::PreHookError` variant
- Delete `PostHookInput::PreHookError` variant
- Delete `PendingTask`, `PendingFinally`, `PendingState` structs

## What doesn't change

- **`run_shell_command`**: Signature and behavior unchanged. Timeout is external (from `run_with_timeout`).
- **Finally hooks**: Already first-class. No changes to `FinallyRun` or finally dispatch.
- **Config types**: `Step.pre` and `Step.post` remain `Option<HookScript>`. No new config fields.
- **`TaskCompleted` structure**: Still records success/failure with children and retry. When a post-hook exists, children in `TaskCompleted` are the raw list (before post-hook modification).
