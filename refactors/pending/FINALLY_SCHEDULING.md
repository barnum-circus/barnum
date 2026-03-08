# Finally Scheduling Refactor

**Status:** Not started

**Prerequisites:** VALUE_AND_RETRY_MODEL (COMPLETED - see `refactors/past/VALUE_AND_RETRY_MODEL.md`)

**Depends on:** FINALLY_TRACKING (COMPLETED - see `refactors/past/FINALLY_TRACKING.md`)

**Blocks:** STATE_PERSISTENCE

## Motivation

Currently finally hooks run synchronously as local shell commands (`sh -c`) immediately when descendants complete. This can't be logged/reconstructed for state persistence.

Change finally to be a regular task that goes through the task pool.

## Current Code Location (as of 2026-03-08)

Finally hooks are implemented in `crates/gsd_config/src/runner/`:
- `finally.rs` - `run_finally_hook_direct()` executes hook synchronously via `run_shell_command()`
- `types.rs` - `Continuation` struct holds `step_name` and `value` for deferred finally execution
- `mod.rs` - `handle_completion()` runs continuations when children complete

The runner uses a unified `BTreeMap<LogTaskId, TaskEntry>` where:
- `TaskEntry` has `parent_id: Option<LogTaskId>` (immediate parent for tree tracking)
- `TaskState::Waiting { pending_count, continuation }` holds deferred finally hooks
- When `pending_count` hits 0, `continuation` (if present) runs via `handle_completion()`

The hook execution uses the shared `run_shell_command()` helper in `runner/shell.rs`.

## Current Behavior

```rust
// runner/finally.rs
pub fn run_finally_hook_direct(
    finally_command: &HookScript,
    value: &serde_json::Value,
) -> Vec<Task> {
    let input_json = serde_json::to_string(value).expect("...");
    match run_shell_command(finally_command.as_str(), &input_json, None) {
        Ok(stdout) => serde_json::from_str(&stdout).unwrap_or_default(),
        Err(_) => vec![],
    }
}

// runner/mod.rs - called from handle_completion()
fn handle_completion(&mut self, task_id: LogTaskId, continuation: Option<Continuation>) {
    let spawned = if let Some(cont) = continuation {
        let hook = self.config.steps.iter()
            .find(|s| s.name == cont.step_name)
            .and_then(|s| s.finally_hook.as_ref())
            .expect("continuation implies finally hook exists");
        run_finally_hook_direct(hook, &cont.value.0)
    } else {
        vec![]
    };
    // Queue spawned tasks as children, or remove task and notify parent
}
```

Problems:
- Synchronous execution blocks the runner
- Local shell command, not through pool
- Can't be logged as TaskSubmitted/TaskCompleted
- If crash during finally, no record of it

## Proposed Behavior

Finally becomes a regular task:

1. Task A completes, spawns B, C, D
2. B, C, D (and all their descendants) complete
3. A's descendant count hits 0 → submit finally task F
4. F goes to back of queue (not prioritized)
5. F runs through task pool like any other task
6. F completes → A is fully done → propagate up

## Data Changes

### TaskSubmitted

Add field to identify finally tasks:

```rust
pub struct TaskSubmitted {
    pub task_id: LogTaskId,
    pub step: String,
    pub value: serde_json::Value,
    pub origin_id: Option<LogTaskId>,
    pub retries: u32,
    pub finally_for: Option<LogTaskId>,  // NEW: if set, this is the finally task for that parent
}
```

### Step Action

Finally hook needs to be invocable through the pool. Options:

1. **New action type**: Add `Finally` variant to action enum
2. **Command action**: Finally is already a shell command, use existing command action type
3. **Special step**: Finally creates a synthetic step with command action

Option 2 seems simplest - finally hook is already a shell command string.

## Flow

### Normal Execution

```
TaskSubmitted { task_id: 1, step: "Analyze", finally_for: None }
TaskCompleted { task_id: 1, outcome: Success }  # spawns 2, 3
TaskSubmitted { task_id: 2, origin_id: 1, finally_for: None }
TaskSubmitted { task_id: 3, origin_id: 1, finally_for: None }
TaskCompleted { task_id: 2, outcome: Success }
TaskCompleted { task_id: 3, outcome: Success }
# Descendants done, submit finally
TaskSubmitted { task_id: 4, origin_id: 1, finally_for: Some(1) }
TaskCompleted { task_id: 4, outcome: Success }
# Task 1 fully done
```

### Resume

On resume, detect tasks needing finally:
- Task completed (has TaskCompleted)
- Task's step has finally hook (from config)
- All descendants done (no pending children recursively)
- No finally task exists (`finally_for: Some(task_id)` not in log)

Submit finally tasks for these, in correct order (deepest first).

## Implementation

1. Remove `run_finally_hook` synchronous execution
2. When descendant count hits 0 and step has finally:
   - Create TaskSubmitted with `finally_for: Some(parent_id)`
   - Queue it (back of queue, not prioritized)
3. Finally task uses command action with the finally hook string
4. Update reconstruct to detect missing finally tasks

## Files Changed

- `crates/gsd_config/src/runner/mod.rs` - change `handle_completion()` to queue finally as a task instead of calling `run_finally_hook_direct()` synchronously
- `crates/gsd_config/src/runner/finally.rs` - keep `run_finally_hook_direct()` but add `create_finally_task()` for async execution
- `crates/gsd_config/src/runner/types.rs` - add `finally_for: Option<LogTaskId>` to `TaskEntry` to track finally tasks
- `crates/gsd_config/src/runner/dispatch.rs` - handle finally task dispatch (uses command action)
- `crates/gsd_config/src/state_log.rs` (new) - add `finally_for` field to TaskSubmitted
