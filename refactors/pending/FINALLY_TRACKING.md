# Finally Tracking Refactor

**Status:** Not started

**Blocks:** FINALLY_SCHEDULING

## Motivation

The current finally tracking algorithm uses a flat structure that can't be reconstructed from the state log. We need a tree-based approach where each task tracks its own children.

## Current Implementation

### Data Structures (Before)

```rust
// runner.rs

struct TaskRunner<'a> {
    // ...
    /// Tracks pending descendants for tasks with `finally` hooks.
    /// Key: origin task ID, Value: (pending count, original value, finally command)
    finally_tracking: HashMap<LogTaskId, FinallyState>,
}

/// Internal task wrapper with lineage tracking.
struct QueuedTask {
    task: Task,
    /// Unique ID for this task instance.
    id: LogTaskId,
    /// If this task descended from a task with `finally`, tracks that origin.
    /// NOTE: This skips intermediate tasks - points directly to finally-ancestor
    origin_id: Option<LogTaskId>,
}

/// State for tracking when a `finally` hook should run.
struct FinallyState {
    /// Number of descendants still pending (in queue or in flight).
    pending_count: usize,
    /// The original task's value (input to finally hook).
    original_value: serde_json::Value,
    /// The finally hook command.
    finally_command: String,
}
```

### Setting Up Finally Tracking (Before)

```rust
// When a task with finally hook spawns children:

let child_origin_id = if finally_hook.is_some() && !final_tasks.is_empty() {
    // Set up finally tracking for this task
    self.finally_tracking.insert(
        task_id,
        FinallyState {
            pending_count: final_tasks.len(),
            original_value: effective_value,
            finally_command: finally_hook.unwrap_or_default(),
        },
    );
    Some(task_id)  // Children point directly to this task
} else {
    origin_id  // Children inherit parent's origin (skip this level)
};

// Queue new tasks - they all point to the finally-ancestor
for new_task in final_tasks {
    self.queue.push_back(QueuedTask {
        task: new_task,
        id: self.next_task_id,
        origin_id: child_origin_id,  // Skips intermediate tasks!
    });
    self.next_task_id += 1;
}
```

### Decrementing on Completion (Before)

```rust
/// Decrement the pending count for an origin and run finally if done.
fn decrement_origin(&mut self, origin_id: Option<LogTaskId>) {
    let Some(oid) = origin_id else { return };

    let should_run_finally = if let Some(state) = self.finally_tracking.get_mut(&oid) {
        state.pending_count = state.pending_count.saturating_sub(1);
        state.pending_count == 0
    } else {
        false
    };

    if should_run_finally && let Some(state) = self.finally_tracking.remove(&oid) {
        self.run_finally_hook(state);
    }
}
```

### Problem

```
A (finally) spawns B, C
B spawns D

Current tracking:
  finally_tracking[A] = { pending: 3 }  // B, C, D all point to A
  B.origin_id = Some(A)
  C.origin_id = Some(A)
  D.origin_id = Some(A)  // Skips B!

On resume, we see:
  D.origin_id = Some(A)

But we can't tell that D is B's child, not A's direct child.
We lose the tree structure.
```

---

## Proposed Implementation

### Data Structures (After)

```rust
// runner.rs

struct TaskRunner<'a> {
    // ...
    /// Per-task state tracking. Tasks not in this map are fully done.
    task_states: HashMap<LogTaskId, TrackedTask>,
}

struct TrackedTask {
    /// Immediate parent (always set except for initial tasks)
    parent_id: Option<LogTaskId>,
    /// Current state of this task
    state: TaskState,
    /// Step name (needed to look up finally hook in config)
    step: StepName,
    /// Original value (passed to finally hook)
    value: serde_json::Value,
}

enum TaskState {
    /// Waiting for agent to complete
    Pending,
    /// Agent done, waiting for N children to fully complete
    AwaitingDescendants(NonZeroU16),
    // Note: When count hits 0, task is removed from task_states (fully done)
}

/// Internal task wrapper - simpler now
struct QueuedTask {
    task: Task,
    id: LogTaskId,
    parent_id: Option<LogTaskId>,  // Always immediate parent, never skips
}
```

### Setting Up Child Tracking (After)

```rust
// When any task spawns children (regardless of finally hook):

for new_task in final_tasks {
    let child_id = self.next_task_id();

    // Track the child
    self.task_states.insert(child_id, TrackedTask {
        parent_id: Some(task_id),  // Always immediate parent
        state: TaskState::Pending,
        step: new_task.step.clone(),
        value: new_task.value.clone(),
    });

    self.queue.push_back(QueuedTask {
        task: new_task,
        id: child_id,
        parent_id: Some(task_id),
    });
}

// Update parent to track child count
if !final_tasks.is_empty() {
    let count = NonZeroU16::new(final_tasks.len() as u16).unwrap();
    if let Some(tracked) = self.task_states.get_mut(&task_id) {
        tracked.state = TaskState::AwaitingDescendants(count);
    }
}
```

### Completion Propagation (After)

```rust
/// Called when a task is fully done (agent complete + all descendants done)
fn task_fully_done(&mut self, task_id: LogTaskId) {
    let tracked = self.task_states.remove(&task_id)
        .expect("task_fully_done called with unknown task_id");

    // Run finally hook if this step has one
    if let Some(finally_cmd) = self.get_finally_hook(&tracked.step) {
        self.run_finally_hook(finally_cmd, tracked.value);
    }

    // Propagate up to parent
    if let Some(parent_id) = tracked.parent_id {
        self.decrement_parent(parent_id);
    }
}

/// Decrement parent's child count, maybe mark parent as fully done
fn decrement_parent(&mut self, parent_id: LogTaskId) {
    let tracked = self.task_states.get_mut(&parent_id)
        .expect("decrement_parent called with unknown parent_id");

    match &mut tracked.state {
        TaskState::AwaitingDescendants(count) => {
            // Decrement count
            let new_count = count.get() - 1;
            if new_count == 0 {
                // Parent is now fully done
                self.task_fully_done(parent_id);
            } else {
                *count = NonZeroU16::new(new_count).unwrap();
            }
        }
        TaskState::Pending => {
            // Shouldn't happen - parent should be AwaitingDescendants
            panic!("Child completed but parent still Pending");
        }
    }
}

// After processing task result and queuing any spawned children:
if spawned.is_empty() {
    // No children - task is immediately fully done
    self.task_fully_done(task_id);
}
// If spawned is non-empty, children were queued above and parent
// state was updated to AwaitingDescendants
```

### Example Trace (After)

```
A (finally) spawns B, C
B spawns D

Initial state:
  task_states[A] = { parent: None, state: AwaitingDescendants(2), step: "X" }
  task_states[B] = { parent: Some(A), state: Pending, step: "Y" }
  task_states[C] = { parent: Some(A), state: Pending, step: "Y" }

B completes, spawns D:
  task_states[A] = { parent: None, state: AwaitingDescendants(2), step: "X" }
  task_states[B] = { parent: Some(A), state: AwaitingDescendants(1), step: "Y" }
  task_states[C] = { parent: Some(A), state: Pending, step: "Y" }
  task_states[D] = { parent: Some(B), state: Pending, step: "Z" }

D completes (no children):
  D fully done → removed from task_states
  decrement B's count: 1→0
  B fully done → removed, run B's finally (none)
  decrement A's count: 2→1

  task_states[A] = { parent: None, state: AwaitingDescendants(1), step: "X" }
  task_states[C] = { parent: Some(A), state: Pending, step: "Y" }

C completes (no children):
  C fully done → removed from task_states
  decrement A's count: 1→0
  A fully done → removed, run A's finally hook

  task_states = {} (empty, all done)
```

---

## Why This Enables Resume

On resume, reconstruct `task_states` from the log:

```rust
fn reconstruct_task_states(entries: &[StateLogEntry]) -> HashMap<LogTaskId, TrackedTask> {
    let mut states = HashMap::new();

    for entry in entries {
        match entry {
            StateLogEntry::TaskSubmitted(t) => {
                states.insert(t.task_id, TrackedTask {
                    parent_id: t.origin_id,
                    state: TaskState::Pending,
                    step: t.step.clone(),
                    value: t.value.clone(),
                });
            }
            StateLogEntry::TaskCompleted(c) => {
                // Count remaining children
                let child_count = states.values()
                    .filter(|t| t.parent_id == Some(c.task_id))
                    .count();

                if child_count == 0 {
                    states.remove(&c.task_id);  // Fully done
                } else {
                    // Update to AwaitingDescendants
                    if let Some(tracked) = states.get_mut(&c.task_id) {
                        tracked.state = TaskState::AwaitingDescendants(
                            NonZeroU16::new(child_count as u16).unwrap()
                        );
                    }
                }
            }
        }
    }

    states
}
```

The tree structure is preserved because `origin_id` (now `parent_id`) always points to immediate parent.

---

## Files Changed

- `crates/gsd_config/src/runner.rs`
  - Remove `finally_tracking: HashMap<LogTaskId, FinallyState>`
  - Add `task_states: HashMap<LogTaskId, TrackedTask>`
  - Change `QueuedTask.origin_id` to `parent_id` (always immediate parent)
  - Rewrite `decrement_origin` → `decrement_parent` + `task_fully_done`
  - Update task spawning to always track parent relationship

## Testing

- Existing finally tests should still pass (behavior unchanged)
- Add test for deeply nested finally (A→B→C→D with finally on A and C)
- Add test for multiple siblings with finally hooks
