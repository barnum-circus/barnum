# Apply Pattern for State/Log Consistency

**Status:** Not started

**Depends on:** STATE_PERSISTENCE (should be implemented as part of or after Phase 3)

## Motivation

Currently, state changes and log writes are separate operations:

```rust
// Two separate operations - easy to forget one
self.log_writer.write(TaskSubmitted { ... });
self.tasks.insert(id, entry);
```

This creates risk of:
1. Forgetting to write to log when changing state
2. Log/state getting out of sync
3. Bugs where state is updated but log isn't (or vice versa)

## Core Idea

All state changes go through a single `apply()` method. But instead of `apply()` doing everything itself, it dispatches to a **registered vector of appliers** — each one handling its own concern independently.

```rust
/// A handler that reacts to state log entries.
trait Applier {
    fn apply(&mut self, entry: &StateLogEntry);
}

impl TaskRunner {
    /// All state changes go through this. Dispatches to registered appliers.
    fn apply(&mut self, entry: StateLogEntry) {
        for applier in &mut self.appliers {
            applier.apply(&entry);
        }
    }
}
```

### Applier Registration

The runner holds a `Vec<Box<dyn Applier>>`. Two appliers ship by default:

```rust
fn build_appliers(log_writer: LogWriter, /* ... */) -> Vec<Box<dyn Applier>> {
    vec![
        Box::new(StateLogApplier { log_writer }),
        Box::new(InternalStateApplier { tasks: HashMap::new(), /* ... */ }),
    ]
}
```

1. **`StateLogApplier`** — writes the entry to the NDJSON state log file. Append-only, no logic.
2. **`InternalStateApplier`** — mutates the in-memory state (task map, pending counts, etc.).

This separation means:
- The log writer doesn't need to understand state transitions
- The state mutator doesn't need to know about logging
- You can add new appliers without touching existing ones (e.g., a metrics applier, a webhook notifier)

### Replay

Replay is just reading an old NDJSON file, parsing each entry, and feeding it through `apply()` — the same `apply()` that runs during normal operation. **All appliers run, including the log applier.** The log applier writes to a *new* log file (the CLI already enforces `--resume-from` and `--state-log` are different files), so the new log gets a clean copy of the old entries followed by any new events from the resumed run.

```rust
fn resume(old_log: &Path, runner: &mut TaskRunner) {
    // Read old log, feed through ALL appliers (including log writer to new file)
    for entry in read_ndjson(old_log) {
        runner.apply(entry);
    }
    // State is reconstructed. New log has a copy of old entries.
    // Now dispatch all Pending tasks and continue normal execution.
}
```

No special replay mode. No `skip_on_replay`. No conditional logic. The trait is just `fn apply(&mut self, entry: &StateLogEntry)` — nothing else.

## Benefits

1. **Impossible to forget logging** — state only changes through `apply()`, which always dispatches to all appliers
2. **Resume uses the exact same code path** — replay feeds old entries through `apply()`, all appliers run, new log gets a copy
3. **Single source of truth** — log entries define what state changes are possible
4. **Testable** — can unit test each applier in isolation
5. **Extensible** — add a metrics applier, a visualization applier, etc. without touching the core

## Derived State: Compute From Log

The log should be minimal — only facts that can't be derived. Everything else is computed.

### What goes in the log

Only raw events:
- `TaskSubmitted` — a task was created with this step, value, parent, origin
- `TaskCompleted` — a task finished with this outcome

### What does NOT go in the log

- `in_flight` count — derived by counting tasks that have been submitted but not completed
- `next_task_id` — derived from `max(task_id) + 1` across all entries
- `pending_children_count` — derived by counting submitted children minus completed children for each parent
- `TaskDispatched` — dispatch (Pending → InFlight) is transient; on resume, re-dispatch all pending tasks

### How `InternalStateApplier` computes derived state

```rust
struct InternalStateApplier {
    tasks: HashMap<LogTaskId, TaskEntry>,
}

impl Applier for InternalStateApplier {
    fn apply(&mut self, entry: &StateLogEntry) {
        match entry {
            StateLogEntry::TaskSubmitted(submitted) => {
                self.tasks.insert(submitted.task_id, TaskEntry {
                    step: submitted.step.clone(),
                    parent_id: submitted.parent_id,
                    state: TaskState::Pending {
                        value: StepInputValue(submitted.value.clone()),
                    },
                    retries_remaining: 0, // resolved later from step config
                });

                // Derived: increment parent's child count
                if let Some(parent_id) = submitted.parent_id {
                    if let Some(parent) = self.tasks.get_mut(&parent_id) {
                        parent.increment_pending_children();
                    }
                }
            }
            StateLogEntry::TaskCompleted(completed) => {
                match &completed.outcome {
                    TaskOutcome::Success(success) => {
                        if success.spawned_task_ids.is_empty() {
                            self.tasks.remove(&completed.task_id);
                            // Derived: decrement parent's child count
                            self.maybe_notify_parent(completed.task_id);
                        } else {
                            // Transition to WaitingForChildren
                            // pending_children_count is derived from how many
                            // TaskSubmitted entries reference this task as parent
                            let entry = self.tasks.get_mut(&completed.task_id).unwrap();
                            entry.state = TaskState::WaitingForChildren {
                                // Count is already tracked from TaskSubmitted events
                            };
                        }
                    }
                    TaskOutcome::Failed(failed) => {
                        self.tasks.remove(&completed.task_id);
                        if failed.retry_task_id.is_none() {
                            self.maybe_notify_parent(completed.task_id);
                        }
                    }
                }
            }
            StateLogEntry::Config(_) => {}
        }
    }
}
```

### Replay is straightforward

Because derived state is computed from log entries and all appliers always run, replay is just reading the old NDJSON and calling `apply()` for each entry:

```rust
fn resume(old_log: &Path, runner: &mut TaskRunner) {
    for entry in read_ndjson(old_log) {
        runner.apply(entry);
        // StateLogApplier writes entry to new log file
        // InternalStateApplier rebuilds in-memory state
    }
    // All derived state (in_flight, pending_children, etc.) is now correct
    // New log file has a copy of all old entries
    // All tasks in Pending state get re-dispatched
}
```

No special replay logic. No "rebuild derived state" pass. No conditional skipping. The same `apply()` code that handles live events also handles replay. The state after replaying N entries is identical to the state after processing N live events.

## Dispatch is Not Logged

Dispatching a task (Pending → InFlight) is not a logged event. It's transient state:
- On resume, all Pending tasks get re-dispatched
- The InFlight state doesn't survive resume anyway
- Logging dispatch would bloat the log with no recovery value

```rust
// dispatch() is separate from apply() - it only changes InFlight status
fn dispatch(&mut self, task_id: LogTaskId) {
    let entry = self.state.tasks.get_mut(&task_id).expect("task exists");
    let value = match &entry.state {
        TaskState::Pending { value } => value.clone(),
        _ => panic!("can only dispatch pending tasks"),
    };
    entry.state = TaskState::InFlight(/* ... */);
    self.spawn_task_future(task_id, value);
}
```

## Implementation

### Before/After: queue_task

```rust
// BEFORE
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>, origin: TaskOrigin) {
    let id = self.next_task_id();

    self.log_writer.write(StateLogEntry::TaskSubmitted(TaskSubmitted {
        task_id: id,
        step: task.step.clone(),
        value: task.value.0.clone(),
        parent_id,
        origin,
    }));

    self.tasks.insert(id, TaskEntry { /* ... */ });

    if self.in_flight < self.max_concurrency {
        self.dispatch(id);
    }
}

// AFTER
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>, origin: TaskOrigin) {
    let id = self.next_task_id();

    // Single call handles logging AND state update via registered appliers
    self.apply(StateLogEntry::TaskSubmitted(TaskSubmitted {
        task_id: id,
        step: task.step.clone(),
        value: task.value.0.clone(),
        parent_id,
        origin,
    }));

    // Dispatch is separate (not logged, not applied)
    if self.in_flight() < self.max_concurrency {
        self.dispatch(id);
    }
}
```

### Before/After: task_succeeded

```rust
// BEFORE
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
    self.in_flight -= 1;

    // ... complex logic ...

    self.log_writer.write(StateLogEntry::TaskCompleted(TaskCompleted {
        task_id,
        outcome: TaskOutcome::Success(TaskSuccess { spawned_task_ids }),
    }));

    // ... more state updates ...
}

// AFTER
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
    // Queue children first (each gets its own TaskSubmitted through apply)
    let spawned_task_ids: Vec<LogTaskId> = spawned.iter().map(|task| {
        let id = self.next_task_id();
        self.apply(StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: id,
            step: task.step.clone(),
            value: task.value.0.clone(),
            parent_id: Some(task_id),
            origin: TaskOrigin::Spawned,
        }));
        id
    }).collect();

    // Then complete the parent (appliers handle WaitingForChildren transition)
    self.apply(StateLogEntry::TaskCompleted(TaskCompleted {
        task_id,
        outcome: TaskOutcome::Success(TaskSuccess { spawned_task_ids }),
    }));
}
```

## Testing

```rust
#[test] fn state_applier_task_submitted_creates_pending_entry()
#[test] fn state_applier_task_submitted_increments_parent_children()
#[test] fn state_applier_task_completed_success_no_children_removes()
#[test] fn state_applier_task_completed_success_with_children_waits()
#[test] fn state_applier_task_completed_failed_with_retry_removes()
#[test] fn state_applier_task_completed_failed_no_retry_notifies_parent()
#[test] fn replay_log_reconstructs_identical_state()
#[test] fn replay_copies_entries_to_new_log()
#[test] fn log_applier_writes_all_entries()
```

## Migration Path

1. Define `Applier` trait
2. Implement `StateLogApplier` (wraps existing log writer)
3. Implement `InternalStateApplier` (extracts existing state mutation logic)
4. Refactor `TaskRunner` to hold `Vec<Box<dyn Applier>>` and dispatch through `apply()`
5. Verify all tests pass
6. Remove dead code paths that did direct mutations or direct log writes
