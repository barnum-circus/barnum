# Apply Pattern for State/Log Consistency

**Status:** Not started

**Depends on:** None

## Motivation

State changes and log writes are separate operations scattered throughout `TaskRunner`. Nothing enforces that they stay in sync:

```rust
self.state_log.write(TaskSubmitted { ... });
self.tasks.insert(id, entry);
```

This creates bugs: missed log writes when state changes, missed state updates when logs are written, and resume logic that silently diverges from live logic.

## Architecture

Troupe's daemon event loop (`crates/troupe/src/daemon/wiring.rs:342`):

```rust
fn run_event_loop(events_rx: Receiver<Event>, effect_tx: Sender<Effect>) -> PoolState {
    let mut state = PoolState::new();
    while let Ok(event) = events_rx.recv() {
        let (new_state, effects) = step(state, event);
        state = new_state;
        for effect in effects {
            if effect_tx.send(effect).is_err() {
                break;
            }
        }
    }
    state
}
```

Barnum's current loop (`crates/barnum_config/src/runner/mod.rs:870`):

```rust
impl Iterator for TaskRunner<'_> {
    type Item = TaskResult;
    fn next(&mut self) -> Option<Self::Item> {
        self.dispatch_all_pending();
        if self.in_flight == 0 { return None; }
        let result = self.rx.recv().ok()?;
        Some(self.process_result(result))
    }
}
```

The structure is already there: receive a result, process it. `process_result` interprets the result (runs post hooks, determines success/failure/retry), then calls `task_succeeded` or `task_failed` which manually write the log and mutate state as separate operations.

Target Barnum event loop:

```rust
let mut appliers: Vec<Box<dyn Applier>> = vec![
    Box::new(StateRunner::new(...)),
    Box::new(LogApplier::new(...)),
];

// seed
let seed_entries = build_seed_entries(...);
for applier in &mut appliers {
    applier.apply(&seed_entries);
}

// event loop
while let Ok(entries) = rx.recv() {
    for applier in &mut appliers {
        applier.apply(&entries);
    }
}
```

The event loop receives entries from the channel and calls apply on each applier in the vector.

StateRunner implements Applier. Its apply iterates entries, matches, updates state, tracks pending dispatches. At the end, it flushes dispatches (spawns threads). If a TaskSubmitted and TaskCompleted for the same task appear in the same batch, the completed removes it from the dispatch queue before flush — no thread is created.

LogApplier implements Applier. Its apply writes each entry to NDJSON.

The current loop already has the same shape: receive, process. The refactor has two separate stages:

1. **Event loop restructure** (Phase 1): Convert the Iterator to an explicit recv loop where `process_result` still handles everything internally. A structural change only.

2. **Apply pattern** (Phase 2): Introduce the Applier trait, build a `Vec<Box<dyn Applier>>`, and have the main loop iterate the vector calling apply on each. Extract step() to produce entries from results.

Resume: construct the applier vector, replay the NDJSON log through apply on each applier, then enter the event loop. Submitted+completed pairs cancel out in the state runner's dispatch queue. Only incomplete tasks get dispatched.

## StateLogEntry

```rust
enum StateLogEntry {
    Config(StateLogConfig),
    TaskSubmitted(TaskSubmitted),
    TaskCompleted(TaskCompleted),
}

struct TaskCompleted {
    task_id: LogTaskId,
    outcome: TaskOutcome,
}

enum TaskOutcome {
    Success(TaskSuccess),
    Failed(TaskFailed),
}

struct TaskSuccess {
    spawned_task_ids: Vec<LogTaskId>,
    finally_value: StepInputValue,
}

struct TaskFailed {
    reason: FailureReason,
    retry_task_id: Option<LogTaskId>,
}
```

Each variant records a fact. RunState derives task removal internally when all children of a parent complete.

## Applier

```rust
trait Applier {
    fn apply(&mut self, entries: &[StateLogEntry]);
}
```

The main loop holds a `Vec<Box<dyn Applier>>` and iterates it, calling apply on each element. StateRunner and LogApplier both implement this trait.

### LogApplier

```rust
struct LogApplier {
    writer: io::BufWriter<File>,
}

impl Applier for LogApplier {
    fn apply(&mut self, entries: &[StateLogEntry]) {
        for entry in entries {
            barnum_state::write_entry(&mut self.writer, entry)
                .expect("failed to write state log entry");
        }
    }
}
```

## RunState

A dependency tracker internal to StateRunner. Tracks tasks and parent-child relationships. StateRunner handles I/O, config lookups, and finally logic; RunState only manages the task tree. When a task completes with no children, it's removed and its parent's child count is decremented. If the count reaches zero, the parent is removed and its own parent's count is decremented, continuing up the tree.

Parents whose count reaches zero are accumulated in `removed_parents`. StateRunner drains this after processing a batch of entries to produce finally tasks.

```rust
struct RunState {
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    next_task_id: u32,
    removed_parents: Vec<RemovedParent>,
}

struct RemovedParent {
    task_id: LogTaskId,
    step: StepName,
    finally_value: StepInputValue,
}

impl RunState {
    fn next_id(&mut self) -> LogTaskId {
        let id = LogTaskId(self.next_task_id);
        self.next_task_id += 1;
        id
    }

    fn apply_submitted(&mut self, submitted: &TaskSubmitted) {
        self.next_task_id = self.next_task_id.max(submitted.task_id.0 + 1);
        self.tasks.insert(submitted.task_id, TaskEntry {
            step: submitted.step.clone(),
            parent_id: submitted.parent_id,
            state: TaskState::Pending {
                value: submitted.value.clone(),
            },
        });
    }

    fn apply_completed(&mut self, completed: &TaskCompleted) {
        match &completed.outcome {
            TaskOutcome::Success(success) => {
                if success.spawned_task_ids.is_empty() {
                    self.remove_and_notify_parent(completed.task_id);
                } else {
                    let count = success.spawned_task_ids.len();
                    let entry = self.tasks.get_mut(&completed.task_id)
                        .expect("completed task must exist");
                    entry.state = TaskState::WaitingForChildren {
                        pending_children_count: NonZeroU16::new(count as u16)
                            .expect("spawned_task_ids is non-empty"),
                        finally_value: success.finally_value.clone(),
                    };
                }
            }
            TaskOutcome::Failed(failed) => {
                if failed.retry_task_id.is_some() {
                    // Retry replaces this task in the parent's child count.
                    // Just remove the failed task without decrementing the parent.
                    self.tasks.remove(&completed.task_id);
                } else {
                    self.remove_and_notify_parent(completed.task_id);
                }
            }
        }
    }

    fn remove_and_notify_parent(&mut self, task_id: LogTaskId) {
        let parent_id = self.tasks.get(&task_id).and_then(|e| e.parent_id);
        self.tasks.remove(&task_id);

        let Some(pid) = parent_id else { return };
        let Some(parent) = self.tasks.get_mut(&pid) else { return };
        let TaskState::WaitingForChildren {
            pending_children_count, finally_value
        } = &mut parent.state else { return };

        match NonZeroU16::new(pending_children_count.get() - 1) {
            Some(n) => *pending_children_count = n,
            None => {
                let step = parent.step.clone();
                let fv = finally_value.clone();
                self.removed_parents.push(RemovedParent {
                    task_id: pid,
                    step,
                    finally_value: fv,
                });
                self.remove_and_notify_parent(pid);
            }
        }
    }

    fn drain_removed_parents(&mut self) -> Vec<RemovedParent> {
        std::mem::take(&mut self.removed_parents)
    }

    fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}
```

### TaskEntry and TaskState

```rust
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    state: TaskState,
}

enum TaskState {
    Pending { value: StepInputValue },
    WaitingForChildren {
        pending_children_count: NonZeroU16,
        finally_value: StepInputValue,
    },
}
```

TaskState has two variants. The current `InFlight` variant is replaced by `in_flight: usize` on StateRunner. `finally_script` and `retries_remaining` are removed from TaskEntry — StateRunner looks up the finally script from `step_map` when needed, and step() determines retry exhaustion by counting Retry-origin siblings for the same parent+step in the task tree.

## step()

Interprets a result and produces `StateLogEntry` values without updating state (except allocating IDs via `next_id()`). The only I/O is `interpret_response`, which runs post hooks and determines success/failure.

```rust
fn step(
    state: &mut RunState,
    config: &Config,
    result: InFlightResult,
) -> Vec<StateLogEntry> {
    let mut entries = Vec::new();
    let task_id = result.task_id;
    let parsed = interpret_response(config, &result);

    match parsed {
        Ok(parsed) => {
            let children: Vec<_> = parsed.next_tasks.iter()
                .map(|t| (state.next_id(), t.clone()))
                .collect();
            let child_ids: Vec<_> = children.iter().map(|(id, _)| *id).collect();

            entries.push(StateLogEntry::TaskCompleted(TaskCompleted {
                task_id,
                outcome: TaskOutcome::Success(TaskSuccess {
                    spawned_task_ids: child_ids,
                    finally_value: parsed.finally_value,
                }),
            }));

            for (id, task) in children {
                entries.push(StateLogEntry::TaskSubmitted(TaskSubmitted {
                    task_id: id,
                    step: task.step,
                    value: task.value,
                    parent_id: Some(task_id),
                    origin: TaskOrigin::Spawned,
                }));
            }
        }
        Err(reason) => {
            let retry_submitted = if should_retry(config, task_id) {
                let retry_id = state.next_id();
                let task = state.tasks.get(&task_id)
                    .expect("task must exist");
                let TaskState::Pending { value } = &task.state else {
                    panic!("retried task must be Pending");
                };
                Some((retry_id, TaskSubmitted {
                    task_id: retry_id,
                    step: task.step.clone(),
                    value: value.clone(),
                    parent_id: task.parent_id,
                    origin: TaskOrigin::Retry,
                }))
            } else {
                None
            };

            entries.push(StateLogEntry::TaskCompleted(TaskCompleted {
                task_id,
                outcome: TaskOutcome::Failed(TaskFailed {
                    reason,
                    retry_task_id: retry_submitted.as_ref().map(|(id, _)| *id),
                }),
            }));

            if let Some((_, submitted)) = retry_submitted {
                entries.push(StateLogEntry::TaskSubmitted(submitted));
            }
        }
    }

    entries
}
```

## StateRunner

Implements `Applier` and owns the `RunState`, dispatch queue (`pending_dispatches`), and pool connection. Its `apply` method updates state based on the entries, generates any finally entries in a loop, and then flushes pending dispatches as threads.

```rust
struct StateRunner {
    state: RunState,
    config: Config,
    step_map: HashMap<StepName, Step>,
    pool: PoolConnection,
    tx: Option<Sender<InFlightResult>>,
    in_flight: usize,
    max_concurrency: usize,
    pending_dispatches: Vec<LogTaskId>,
}

impl Applier for StateRunner {
    fn apply(&mut self, entries: &[StateLogEntry]) {
        let mut pending = entries.to_vec();
        while !pending.is_empty() {
            for entry in &pending {
                match entry {
                    StateLogEntry::TaskSubmitted(s) => {
                        self.state.apply_submitted(s);
                        self.pending_dispatches.push(s.task_id);
                    }
                    StateLogEntry::TaskCompleted(c) => {
                        self.state.apply_completed(c);
                        self.pending_dispatches.retain(|id| *id != c.task_id);
                    }
                    StateLogEntry::Config(_) => {}
                }
            }
            pending = self.finally_entries();
        }
        self.flush_dispatches();
    }
}

impl StateRunner {
    fn finally_entries(&mut self) -> Vec<StateLogEntry> {
        self.state.drain_removed_parents().into_iter().filter_map(|parent| {
            let script = self.step_map.get(&parent.step)
                .and_then(|s| s.finally.as_ref())?;
            let id = self.state.next_id();
            Some(StateLogEntry::TaskSubmitted(TaskSubmitted {
                task_id: id,
                step: script.step.clone(),
                value: parent.finally_value,
                parent_id: None,
                origin: TaskOrigin::Finally,
            }))
        }).collect()
    }

    fn flush_dispatches(&mut self) {
        let Some(tx) = &self.tx else { return };

        while self.in_flight < self.max_concurrency {
            let Some(task_id) = self.pending_dispatches.pop() else { break };
            let entry = match self.state.tasks.get(&task_id) {
                Some(e) => e,
                None => continue,
            };
            let value = match &entry.state {
                TaskState::Pending { value } => value.clone(),
                _ => continue,
            };
            self.in_flight += 1;
            let tx = tx.clone();
            // spawn worker thread with tx clone
        }

        if self.state.is_empty() && self.in_flight == 0 {
            self.tx = None;
        }
    }
}
```

### Termination

Worker threads hold `Sender<InFlightResult>` clones. They drop them after sending their result. StateRunner drops its sender when state is empty and `in_flight` is 0. With all senders dropped, `rx.recv()` returns `Err` and the loop exits.

## Usage

```rust
pub fn run(
    config: Config,
    initial_tasks: Vec<Task>,
    runner_config: &RunnerConfig,
) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();

    let mut appliers: Vec<Box<dyn Applier>> = vec![
        Box::new(StateRunner::new(&config, runner_config, tx)?),
        Box::new(LogApplier::new(&runner_config.state_log_path)?),
    ];

    let seed_entries = build_seed_entries(&config, &initial_tasks);
    for applier in &mut appliers {
        applier.apply(&seed_entries);
    }

    while let Ok(entries) = rx.recv() {
        for applier in &mut appliers {
            applier.apply(&entries);
        }
    }

    Ok(())
}
```

Resume: construct the same applier vector, replay the existing NDJSON log through apply on each applier, then enter the event loop. Submitted+completed pairs cancel out in the state runner's dispatch queue — only incomplete tasks get dispatched.

## Phasing

Each phase is a separate branch that passes CI and merges independently.

### Phase 0: Data structure cleanup

Independent refactors that can land in any order.

**0a.** Extract `RunState` from `TaskRunner`. Move `tasks: BTreeMap<LogTaskId, TaskEntry>` and `next_task_id: u32` into a `RunState` struct with its own `apply()` method. `TaskRunner` holds `state: RunState`. Pure structural move.

**0b.** Remove `InFlight` from `TaskState`. Dispatched tasks stay `Pending`. Track dispatch count with `in_flight: usize` on TaskRunner.

**0c.** Remove config fields from `TaskEntry`. Drop `finally_script` and `retries_remaining`. Look them up from `step_map` when needed.

**0d.** Make parent removal derived. When the last child completes, remove the parent inside `apply()` and capture the removed parent's info in `removed_parents: Vec<RemovedParent>`. The runner drains this to check config for finally scripts.

### Phase 1: Event loop restructure

**Depends on: None (can run in parallel with Phase 0).**

Convert the Iterator-based loop to an explicit `run()` method with a recv loop. `process_result` still handles everything internally, and the Applier trait isn't introduced yet. The only change is the loop shape: `while let Ok(result) = self.rx.recv() { self.process_result(result); }`.

### Phase 2: Apply pattern

**Depends on: Phase 0a, Phase 1.**

Introduce the `Applier` trait. StateRunner and LogApplier both implement it. Build a `Vec<Box<dyn Applier>>`. The main loop receives entries from the channel and iterates the vector calling apply on each. step() produces entries before they reach the channel, outside the event loop.

### Phase 3: Seeding through apply

**Depends on: Phase 2.**

Restructure `run()` so seed entries go through the applier vector. `build_seed_entries` produces entries. The main loop iterates the vector and calls apply on each, same as the event loop.

## Before/After

### Task success: log and state are separate operations

Before (`runner/mod.rs:695`): `task_succeeded` manually writes the log, then separately mutates state. Miss either one and they diverge.

```rust
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
    self.in_flight -= 1;
    let entry = self.tasks.get(&task_id).expect("task must exist");
    let finally_hook = self.lookup_finally_hook(entry);

    if spawned.is_empty() {
        self.write_log(&StateLogEntry::TaskCompleted(TaskCompleted {  // 1. write log
            task_id,
            outcome: TaskOutcome::Success(TaskSuccess {
                spawned_task_ids: vec![],
                finally_value: value.clone(),
            }),
        }));
        if let Some(hook) = finally_hook {
            self.schedule_finally(task_id, hook, value);               // 2. mutate state (finally)
        }
        self.remove_and_notify_parent(task_id);                        // 3. mutate state (remove)
    } else {
        // ... compute child IDs, write log, mutate state, queue children ...
    }
}
```

After: `step()` produces entries. The main loop iterates the applier vector and calls apply on each.

```rust
// step() returns entries:
entries.push(StateLogEntry::TaskCompleted(TaskCompleted { task_id, outcome: ... }));
for (id, task) in children {
    entries.push(StateLogEntry::TaskSubmitted(TaskSubmitted { task_id: id, ... }));
}

// main loop:
for applier in &mut appliers {
    applier.apply(&entries);
}
```

### Finally handling: config baked into state

Before (`runner/mod.rs:580`): `schedule_finally` stores `HookScript` on the task entry and increments the parent's child count. RunState knows about config.

```rust
fn schedule_finally(&mut self, task_id: LogTaskId, hook: HookScript, value: StepInputValue) {
    let entry = self.tasks.get(&task_id).expect("task must exist");
    let parent_id = entry.parent_id;
    if let Some(parent_id) = parent_id {
        self.increment_pending_children(parent_id);  // mutate parent in-place
    }
    let id = self.next_task_id();
    self.write_log(&StateLogEntry::TaskSubmitted(TaskSubmitted {
        task_id: id, step: step.clone(), value: value.clone(),
        parent_id,
        origin: TaskOrigin::Finally { finally_for: task_id },
    }));
    self.tasks.insert(id, TaskEntry {
        finally_script: Some(hook),  // config stored on entry
        retries_remaining,           // config stored on entry
        ..
    });
}
```

After: RunState has no config awareness. When a parent's children all complete, it's removed and captured in `removed_parents`. StateRunner drains these inside its Applier::apply, creates TaskSubmitted entries, and feeds them back through the same apply loop so they're handled identically to any other entries. See the StateRunner section for the full implementation.

### Main loop: scattered responsibilities

Before (`runner/mod.rs:870`): Iterator trait with dispatch and processing mixed together.

```rust
impl Iterator for TaskRunner<'_> {
    type Item = TaskResult;
    fn next(&mut self) -> Option<Self::Item> {
        self.dispatch_all_pending();
        if self.in_flight == 0 { return None; }
        let result = self.rx.recv().ok()?;
        Some(self.process_result(result))  // calls task_succeeded/task_failed internally
    }
}
```

After: receive entries, iterate appliers.

```rust
while let Ok(entries) = rx.recv() {
    for applier in &mut appliers {
        applier.apply(&entries);
    }
}
```

### TaskEntry: config fields on every entry

Before (`runner/mod.rs:77`):

```rust
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    finally_script: Option<HookScript>,  // config
    state: TaskState,
    retries_remaining: u32,              // config
}
```

After:

```rust
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    state: TaskState,
}
```

`finally_script` and `retries_remaining` are looked up from `step_map` when needed.

## Testing

```rust
// RunState (pure dependency tracker, no I/O)
#[test] fn apply_submitted_creates_pending_entry()
#[test] fn apply_submitted_advances_next_task_id()
#[test] fn apply_completed_success_no_children_removes_task()
#[test] fn apply_completed_success_with_children_transitions_to_waiting()
#[test] fn apply_completed_child_decrements_parent_count()
#[test] fn apply_completed_last_child_removes_parent()
#[test] fn apply_completed_last_child_captures_removed_parent()
#[test] fn apply_completed_recursive_removal_up_tree()
#[test] fn apply_completed_failed_removes_task()

// LogApplier
#[test] fn writes_all_entry_variants()

// step()
#[test] fn step_success_produces_completed_then_children()
#[test] fn step_failure_produces_completed()
#[test] fn step_failure_with_retry_produces_completed_then_submitted()

// StateRunner::apply()
#[test] fn apply_updates_state_and_log()
#[test] fn apply_queues_submitted_tasks_for_dispatch()
#[test] fn apply_dequeues_completed_tasks()
#[test] fn apply_handles_removed_parents_finally()
#[test] fn apply_skips_finally_when_no_script()

// StateRunner::flush_dispatches()
#[test] fn flush_dispatches_up_to_max_concurrency()
#[test] fn flush_skips_completed_tasks()
#[test] fn flush_drops_tx_when_empty_and_no_in_flight()

// Resume
#[test] fn replay_log_reconstructs_identical_state()
#[test] fn replay_log_only_dispatches_incomplete_tasks()
```
