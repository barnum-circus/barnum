# Finally Scheduling Refactor

**Status:** Not started

**Prerequisites:** VALUE_AND_RETRY_MODEL (COMPLETED), FINALLY_TRACKING (COMPLETED)

**Blocks:** STATE_PERSISTENCE (partially - persistence can work without this, but finally won't be logged)

---

## Bug: Synchronous Finally Blocks Concurrency

### The Problem

Currently, `handle_completion()` calls `run_finally_hook_direct()` **synchronously**. This blocks the entire runner loop, preventing other tasks from being dispatched even when concurrency slots are available.

### Reproduction Scenario

**Setup:**
- `max_concurrency = 1`
- Task A has a finally hook
- Task B has no finally hook
- Initial tasks: `[A, B]`

**Current (buggy) temporal trace:**

```
Time   Event                              in_flight  pending   Notes
─────────────────────────────────────────────────────────────────────────
t0     runner.next() called               0          [A, B]
t1     dispatch_all_pending()             0          [A, B]
t2       dispatch(A)                      1          [B]       A starts
t3     rx.recv() blocks...                1          [B]       waiting for A
t4     A completes, recv() returns        1          [B]
t5     process_result(A)                  1          [B]
t6       task_succeeded(A)                1          [B]
t7         handle_completion(A, Some)     1          [B]
t8           run_finally_hook_direct()    1          [B]       ← BLOCKS HERE
             ...finally runs...           1          [B]       B still waiting!
t9           finally returns              1          [B]
t10        spawned.is_empty() → remove A  0          [B]       slot freed
t11    return from process_result         0          [B]
t12    runner.next() called again         0          [B]
t13    dispatch_all_pending()             0          [B]
t14      dispatch(B)                      1          []        B finally starts
t15    rx.recv() blocks...                1          []
t16    B completes                        0          []
t17    done                               0          []

Observed order: A_done, A_finally, B_done
```

**Key problem:** Between t8 and t9, the runner is blocked running the finally hook. B cannot start even though A's action is complete and the concurrency slot should be free.

### Expected (fixed) temporal trace:

```
Time   Event                              in_flight  pending   Notes
─────────────────────────────────────────────────────────────────────────
t0     runner.next() called               0          [A, B]
t1     dispatch_all_pending()             0          [A, B]
t2       dispatch(A)                      1          [B]       A starts
t3     rx.recv() blocks...                1          [B]
t4     A completes, recv() returns        1          [B]
t5     process_result(A)                  1          [B]
t6       task_succeeded(A)                1          [B]
t7         handle_completion(A, Some)     1          [B]
t8           queue finally task F         1          [B, F]    F queued, not run
t9           A → Waiting{count:1}         0          [B, F]    slot freed!
t10    return from process_result         0          [B, F]
t11    runner.next() called again         0          [B, F]
t12    dispatch_all_pending()             0          [B, F]
t13      dispatch(B)                      1          [F]       B starts immediately
t14    rx.recv() blocks...                1          [F]
t15    B completes                        0          [F]
t16    dispatch_all_pending()             0          [F]
t17      dispatch(F)                      1          []        finally starts
t18    rx.recv() blocks...                1          []
t19    F completes                        1          []
t20      decrement_parent(A)              0          []        A done
t21    done                               0          []

Observed order: A_done, B_done, A_finally
```

**Key difference:** At t9, instead of blocking, we queue F and free the concurrency slot. B can start at t13 while A waits for F.

---

## Test Cases

### Test 1: `finally_should_not_block_concurrency`

**File:** `crates/gsd_config/tests/finally_retry_bugs.rs`

```rust
/// Bug: Synchronous finally blocks other tasks from starting.
///
/// Setup: max_concurrency=1, tasks [A, B] where A has finally
/// Expected order: A_done, B_done, A_finally (B starts while A waits for finally)
/// Actual (buggy): A_done, A_finally, B_done (finally blocks B)
#[test]
#[should_panic(expected = "wrong order")]
fn finally_should_not_block_concurrency() {
    // Config:
    // - max_concurrency: 1
    // - StepA: action completes, has finally hook that records "A_finally"
    // - StepB: action completes, records "B_done", no finally
    //
    // Initial tasks: [A, B]
    //
    // We use the mock pool's ability to control completion order.
    // Both A and B complete their actions quickly.
    // The finally hook also completes quickly.
    // The question is: does B start before or after A's finally runs?
}
```

### Test 2: `finally_task_appears_in_task_tree`

```rust
/// Finally task F should be a child of A in the task tree.
///
/// Setup: A has finally that spawns C
/// Tree should be: A → F → C (not A → C)
#[test]
#[should_panic(expected = "wrong parent")]
fn finally_task_appears_in_task_tree() {
    // Verify that when finally spawns tasks, they are children of F, not A
}
```

### Test 3: `finally_task_failure_handling`

```rust
/// What happens when a finally task fails?
///
/// Current behavior: finally failures are silently ignored
/// Expected behavior: TBD - probably still ignored, but logged
#[test]
fn finally_task_failure_is_logged() {
    // Verify finally failures don't crash the runner
    // Verify parent task still completes (finally failure doesn't block)
}
```

---

## Implementation Details

### Data Structure Changes

#### `types.rs` - Add finally task marker

```rust
// BEFORE:
pub(super) struct TaskEntry {
    pub parent_id: Option<LogTaskId>,
    pub state: TaskState,
}

// AFTER:
pub(super) struct TaskEntry {
    pub parent_id: Option<LogTaskId>,
    pub state: TaskState,
    /// If this task is a finally task, which task is it finally for?
    /// Used for logging/debugging, not for tree structure (parent_id handles that).
    pub finally_for: Option<LogTaskId>,
}
```

#### `types.rs` - Finally task identity

Finally tasks need a way to identify what to run. Options:

**Option A: Store hook script in task value**
```rust
// Finally task has special step name and value contains the script
Task {
    step: StepName::new("__finally__"),
    value: json!({ "script": "./cleanup.sh", "input": original_value }),
}
```

**Option B: Synthetic step created at queue time**
```rust
// Create a temporary Command step for the finally hook
// Pro: Uses existing dispatch path
// Con: Step doesn't exist in config, validation issues?
```

**Option C: Special dispatch path for finally tasks**
```rust
// Check finally_for field and dispatch differently
// Pro: Clean separation
// Con: Another code path to maintain
```

**Recommendation:** Option A - store script in value, use sentinel step name `__finally__`. Dispatch checks for this and runs as command.

### Code Changes

#### `mod.rs` - `handle_completion()` changes

```rust
// BEFORE (synchronous):
fn handle_completion(&mut self, task_id: LogTaskId, continuation: Option<Continuation>) {
    let spawned = if let Some(cont) = continuation {
        let hook = self.config.steps.iter()
            .find(|s| s.name == cont.step_name)
            .and_then(|s| s.finally_hook.as_ref())
            .expect("continuation implies finally hook exists");
        run_finally_hook_direct(hook, &cont.value.0)  // ← BLOCKS
    } else {
        vec![]
    };

    if spawned.is_empty() {
        // Remove and notify parent
        let entry = self.tasks.remove(&task_id).expect("task must exist");
        if matches!(entry.state, TaskState::InFlight(_)) {
            self.in_flight -= 1;
        }
        if let Some(parent_id) = entry.parent_id {
            self.decrement_parent(parent_id);
        }
    } else {
        // Transition to Waiting for spawned tasks
        // ...
    }
}

// AFTER (async):
fn handle_completion(&mut self, task_id: LogTaskId, continuation: Option<Continuation>) {
    if let Some(cont) = continuation {
        // Queue finally as a task instead of running synchronously
        let finally_task = self.create_finally_task(&cont);

        // Transition to Waiting for the finally task
        let entry = self.tasks.get_mut(&task_id).expect("task must exist");
        if matches!(entry.state, TaskState::InFlight(_)) {
            self.in_flight -= 1;
        }
        entry.state = TaskState::Waiting {
            pending_count: NonZeroU16::new(1).unwrap(),
            continuation: None,  // continuation consumed
        };

        // Queue finally task as child of this task
        self.queue_finally_task(finally_task, task_id);
        return;
    }

    // No continuation - remove and notify parent (unchanged)
    let entry = self.tasks.remove(&task_id).expect("task must exist");
    if matches!(entry.state, TaskState::InFlight(_)) {
        self.in_flight -= 1;
    }
    if let Some(parent_id) = entry.parent_id {
        self.decrement_parent(parent_id);
    }
}

fn create_finally_task(&self, cont: &Continuation) -> Task {
    let hook = self.config.steps.iter()
        .find(|s| s.name == cont.step_name)
        .and_then(|s| s.finally_hook.as_ref())
        .expect("continuation implies finally hook exists");

    Task {
        step: StepName::new("__finally__"),
        value: serde_json::json!({
            "script": hook.as_str(),
            "input": cont.value.0,
        }),
    }
}

fn queue_finally_task(&mut self, task: Task, finally_for: LogTaskId) {
    let id = self.next_task_id();

    if self.in_flight < self.max_concurrency {
        self.dispatch_finally(id, task, finally_for);
    } else {
        let prev = self.tasks.insert(
            id,
            TaskEntry {
                parent_id: Some(finally_for),
                state: TaskState::Pending(task),
                finally_for: Some(finally_for),
            },
        );
        assert!(prev.is_none());
    }
}
```

#### `mod.rs` - `dispatch()` changes for finally tasks

```rust
fn dispatch(&mut self, task_id: LogTaskId, task: Task, parent_id: Option<LogTaskId>) {
    // Check for finally task sentinel
    if task.step.as_str() == "__finally__" {
        self.dispatch_finally_task(task_id, task, parent_id);
        return;
    }

    // Normal dispatch path (unchanged)
    // ...
}

fn dispatch_finally_task(
    &mut self,
    task_id: LogTaskId,
    task: Task,
    parent_id: Option<LogTaskId>,
) {
    let script = task.value["script"].as_str().expect("finally task must have script");
    let input = &task.value["input"];

    let tx = self.tx.clone();
    let working_dir = self.pool.working_dir.clone();
    let script = script.to_string();
    let input_json = serde_json::to_string(input).expect("input serializes");

    let identity = TaskIdentity { task, task_id };

    info!(task_id = ?task_id, "dispatching finally task");

    thread::spawn(move || {
        let result = run_shell_command(&script, &input_json, None);
        let submit_result = match result {
            Ok(stdout) => {
                // Parse spawned tasks from stdout
                let spawned: Vec<Task> = serde_json::from_str(&stdout).unwrap_or_default();
                SubmitResult::Finally { spawned }
            }
            Err(e) => {
                warn!(error = %e, "finally task failed");
                SubmitResult::Finally { spawned: vec![] }
            }
        };
        tx.send(InFlightResult { identity, result: submit_result }).ok();
    });

    let prev = self.tasks.insert(
        task_id,
        TaskEntry {
            parent_id,
            state: TaskState::InFlight(InFlight::new()),
            finally_for: parent_id,  // finally_for == parent_id for finally tasks
        },
    );
    assert!(prev.is_none());
    self.in_flight += 1;
}
```

#### `types.rs` - Add Finally variant to SubmitResult

```rust
pub(super) enum SubmitResult {
    Pool {
        effective_value: EffectiveValue,
        response: io::Result<Response>,
    },
    Command {
        effective_value: EffectiveValue,
        output: io::Result<String>,
    },
    PreHookError(String),
    Finally {  // NEW
        spawned: Vec<Task>,
    },
}
```

#### `response.rs` - Handle Finally result

```rust
pub(super) fn process_submit_result(
    result: SubmitResult,
    task: &Task,
    step: &Step,
    schemas: &CompiledSchemas,
) -> ProcessedSubmit {
    match result {
        SubmitResult::Finally { spawned } => {
            // Finally tasks don't go through normal processing
            // They just return spawned tasks (or empty)
            ProcessedSubmit {
                outcome: TaskOutcome::Success {
                    spawned,
                    finally_value: EffectiveValue(task.value.clone()),
                },
                post_input: PostHookInput::Success {
                    input: task.value.clone(),
                    output: serde_json::Value::Null,
                    next: spawned,
                },
            }
        }
        // ... existing cases unchanged
    }
}
```

---

## Task Tree Structure

### Before (current)

When A has finally that spawns C:
```
A completes
  → run_finally_hook_direct() SYNC
  → returns [C]
  → C queued as child of A

Tree:
A (Waiting for C)
└── C
```

### After (fixed)

When A has finally that spawns C:
```
A completes
  → queue finally task F as child of A
  → A waits for F

F dispatched
  → runs finally script
  → returns [C]
  → C queued as child of F
  → F waits for C

C completes
  → F.pending_count → 0 → F done
  → A.pending_count → 0 → A done

Tree:
A (Waiting for F)
└── F (finally task, Waiting for C)
    └── C
```

This is cleaner - finally is just another task in the tree.

---

## State Persistence Integration

### TaskSubmitted for finally tasks

```json
{"kind":"TaskSubmitted","task_id":4,"step":"__finally__","value":{"script":"./cleanup.sh","input":{...}},"parent_id":1,"finally_for":1}
```

### Resume logic

On resume, detect tasks needing finally:
1. Find all TaskCompleted entries
2. For each, check if step has finally hook (from config)
3. Check if finally task exists (TaskSubmitted with `finally_for: Some(task_id)`)
4. If not, and all descendants done, queue finally task

```rust
fn detect_missing_finally_tasks(
    config: &Config,
    completed: &HashSet<LogTaskId>,
    finally_submitted: &HashMap<LogTaskId, LogTaskId>,  // finally_for → task_id
) -> Vec<(LogTaskId, HookScript)> {
    let mut missing = vec![];

    for task_id in completed {
        // Check if this task's step has a finally hook
        let step = get_step_for_task(task_id);  // need to track this
        if let Some(hook) = &step.finally_hook {
            // Check if finally was already submitted
            if !finally_submitted.contains_key(task_id) {
                missing.push((*task_id, hook.clone()));
            }
        }
    }

    // Sort by depth (deepest first) to run in correct order
    missing.sort_by_key(|(id, _)| depth_of(*id));
    missing
}
```

---

## Edge Cases

### 1. Finally task times out

Currently not possible (shell commands don't have timeouts). After this change, finally tasks could have timeouts if we add that feature. For now, finally tasks run without timeout.

### 2. Finally task fails

Current behavior: failure is silently ignored, parent still completes.
New behavior: same - failure is logged but parent still completes. Finally is best-effort.

### 3. Crash during finally task

Before: finally runs synchronously, crash = no record, finally might re-run on resume.
After: finally is a task, TaskSubmitted logged. On resume, we see finally was submitted but not completed, so we re-run it.

### 4. Finally spawns tasks that fail

Same as any parent whose children fail - parent's pending_count decrements, parent eventually completes.

---

## Implementation Checklist

### Phase 1: Add test documenting the bug

- [ ] Add `finally_should_not_block_concurrency` test to `finally_retry_bugs.rs`
- [ ] Test should `#[should_panic]` with current implementation
- [ ] Commit with `--no-verify` (test skips in sandbox)

### Phase 2: Data structure changes

- [ ] Add `finally_for: Option<LogTaskId>` to `TaskEntry`
- [ ] Add `SubmitResult::Finally { spawned: Vec<Task> }` variant
- [ ] Update all `TaskEntry` construction sites

### Phase 3: Queue finally as task

- [ ] Modify `handle_completion()` to queue finally task instead of running sync
- [ ] Add `create_finally_task()` helper
- [ ] Add `queue_finally_task()` helper
- [ ] Add `dispatch_finally_task()` for finally-specific dispatch

### Phase 4: Process finally results

- [ ] Handle `SubmitResult::Finally` in `process_submit_result()`
- [ ] Spawned tasks from finally become children of finally task (not original task)

### Phase 5: Verify and clean up

- [ ] Remove `#[should_panic]` from test
- [ ] Verify all existing tests pass
- [ ] Update STATE_PERSISTENCE.md to note finally is now loggable

---

## Files Changed Summary

| File | Changes |
|------|---------|
| `runner/types.rs` | Add `finally_for` to `TaskEntry`, add `SubmitResult::Finally` |
| `runner/mod.rs` | Modify `handle_completion()`, add `create_finally_task()`, `queue_finally_task()`, `dispatch_finally_task()` |
| `runner/response.rs` | Handle `SubmitResult::Finally` |
| `tests/finally_retry_bugs.rs` | Add `finally_should_not_block_concurrency` test |
