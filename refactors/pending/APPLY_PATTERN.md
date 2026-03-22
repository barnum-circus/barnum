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

### Design invariant: IDs come from entries

Every applier sees task IDs only through `StateLogEntry` values. Appliers read IDs from the entries they receive; they never allocate IDs. The StateApplier is the sole source of new IDs — it allocates them internally and embeds them in the entries it emits.

This is required for resume correctness. During resume, the NDJSON log is replayed through the same appliers. If an applier allocated its own IDs instead of reading them from entries, the replayed IDs would diverge from the log — state and log would disagree, which is the bug this refactor exists to fix. The StateApplier advances its ID counter during replay by reading IDs from log entries, so live-mode allocations pick up where the log left off.

### Target event loop

The coordinator owns a `Vec<Box<dyn Applier>>`. That's it. It has no knowledge of RunState, channels, config, or any other internal detail. All behavior lives inside the appliers and is accessed through the trait interface.

```rust
pub fn run(
    config: &Config,
    initial_tasks: Vec<Task>,
    runner_config: &RunnerConfig,
) -> io::Result<()> {
    let (tx, rx) = mpsc::channel::<InFlightResult>();

    let mut appliers: Vec<Box<dyn Applier>> = vec![
        Box::new(StateApplier::new(rx, config)),
        Box::new(Dispatcher::new(tx, &runner_config.pool, runner_config.max_concurrency)),
        Box::new(LogApplier::new(&runner_config.state_log_path)?),
    ];

    // seed
    let seed_entries = build_seed_entries(config, &initial_tasks);
    apply_all(&mut appliers, &seed_entries);

    // start (enables live operation after optional replay)
    for applier in appliers.iter_mut() {
        applier.start();
    }

    // event loop
    loop {
        let mut entries = Vec::new();
        for applier in appliers.iter_mut() {
            if let Some(new) = applier.recv() {
                entries.extend(new);
            }
        }
        if entries.is_empty() { break; }
        apply_all(&mut appliers, &entries);
    }

    Ok(())
}

fn apply_all(appliers: &mut [Box<dyn Applier>], entries: &[StateLogEntry]) {
    let mut pending = entries.to_vec();
    while !pending.is_empty() {
        let mut new = Vec::new();
        for applier in appliers.iter_mut() {
            new.extend(applier.apply(&pending));
        }
        pending = new;
    }
}
```

The coordinator is a dumb loop: ask appliers for new entries via `recv()`, feed them through `apply_all()` which iterates until stable, repeat. Adding or removing appliers requires no changes to the event loop.

The current loop already has the same shape: receive, process. The refactor has two separate stages:

1. **Event loop restructure** (Phase 1): Convert the Iterator to an explicit recv loop where `process_result` still handles everything internally. A structural change only.

2. **Apply pattern** (Phase 2): Introduce the Applier trait. Build a `Vec<Box<dyn Applier>>`. The coordinator becomes the dumb loop above.

### Resume

Replay the NDJSON log through `apply_all`. During replay, entries are applied to all appliers: the StateApplier rebuilds its task tree, the Dispatcher queues incomplete tasks (TaskSubmitted without matching TaskCompleted cancel out), and the LogApplier is either skipped or in a no-write mode.

After replay, `start()` is called on all appliers. The Dispatcher enables flushing and dispatches its pending queue. The StateApplier enables `recv()` (which blocks on the channel). Then the event loop begins.

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

Each variant records a fact. The StateApplier derives task removal internally when all children of a parent complete.

## Applier

```rust
trait Applier {
    /// Process entries. Returns new entries to feed back through all appliers.
    fn apply(&mut self, entries: &[StateLogEntry]) -> Vec<StateLogEntry>;

    /// Block until external input produces new entries.
    /// Returns None when no more input is expected (termination signal).
    /// Default: no external input.
    fn recv(&mut self) -> Option<Vec<StateLogEntry>> { None }

    /// Called after replay, before the live event loop begins.
    fn start(&mut self) {}
}
```

The coordinator holds a `Vec<Box<dyn Applier>>` and interacts with it exclusively through this trait. `apply_all` feeds entries through all appliers and loops until no applier produces new entries. `recv()` is the blocking point for external input — only one applier (StateApplier) actually blocks; others return `None`.

### StateApplier

Wraps RunState and owns the channel receiver, config reference, and ID allocator. All entry production — child tasks, retries, finally tasks — happens inside this applier and flows back through the trait interface.

```rust
struct StateApplier<'a> {
    state: RunState,
    config: &'a Config,
    rx: Receiver<InFlightResult>,
    live: bool,
}

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
```

**`recv()`**: Blocks on the channel, interprets the result (runs post hooks via config), allocates IDs, and returns entries. This absorbs the current `step()` function.

```rust
fn recv(&mut self) -> Option<Vec<StateLogEntry>> {
    if !self.live { return None; }
    let result = self.rx.recv().ok()?;
    Some(self.interpret_and_produce_entries(result))
}
```

`interpret_and_produce_entries` runs post hooks to determine success/failure, then:
- **Success**: allocates IDs for child tasks, returns `TaskCompleted` + `TaskSubmitted` entries.
- **Failure with retry**: looks up task info from `state.tasks`, allocates a retry ID, returns `TaskCompleted` + `TaskSubmitted`.
- **Failure permanent**: returns `TaskCompleted`.

**`apply()`**: Applies entries to RunState, then drains `removed_parents` to produce finally entries.

```rust
fn apply(&mut self, entries: &[StateLogEntry]) -> Vec<StateLogEntry> {
    for entry in entries {
        match entry {
            StateLogEntry::TaskSubmitted(s) => self.state.apply_submitted(s),
            StateLogEntry::TaskCompleted(c) => self.state.apply_completed(c),
            StateLogEntry::Config(_) => {}
        }
    }
    self.build_finally_entries()
}
```

**`build_finally_entries()`**: Drains `removed_parents`, looks up each step's finally script from config, allocates an ID, and returns `TaskSubmitted` entries. These entries flow back through all appliers (including this one) via `apply_all`'s loop.

```rust
fn build_finally_entries(&mut self) -> Vec<StateLogEntry> {
    self.state.drain_removed_parents().into_iter().filter_map(|parent| {
        let script = self.config.step_map.get(&parent.step)
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
```

**`start()`**: Sets `self.live = true`.

**RunState internals** (unchanged from EXTRACT_RUN_STATE):

```rust
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
}
```

### LogApplier

```rust
struct LogApplier {
    writer: io::BufWriter<File>,
    live: bool,
}

impl Applier for LogApplier {
    fn apply(&mut self, entries: &[StateLogEntry]) -> Vec<StateLogEntry> {
        if self.live {
            for entry in entries {
                barnum_state::write_entry(&mut self.writer, entry)
                    .expect("failed to write state log entry");
            }
        }
        vec![]
    }

    fn start(&mut self) {
        self.live = true;
    }
}
```

During replay, `live` is false and entries are not written (they came from the log). After `start()`, new entries are written.

### Dispatcher

Tracks pending dispatches and spawns worker threads. Gets all task information (step, value) from `TaskSubmitted` entries. Tracks which tasks were actually dispatched to distinguish worker completions from replay completions when decrementing `in_flight`.

```rust
struct Dispatcher {
    pool: PoolConnection,
    tx: Option<Sender<InFlightResult>>,
    in_flight: usize,
    max_concurrency: usize,
    pending_dispatches: VecDeque<PendingTask>,
    dispatched: HashSet<LogTaskId>,
    live: bool,
}

struct PendingTask {
    task_id: LogTaskId,
    step: StepName,
    value: StepInputValue,
}

impl Applier for Dispatcher {
    fn apply(&mut self, entries: &[StateLogEntry]) -> Vec<StateLogEntry> {
        for entry in entries {
            match entry {
                StateLogEntry::TaskSubmitted(s) => {
                    self.pending_dispatches.push_back(PendingTask {
                        task_id: s.task_id,
                        step: s.step.clone(),
                        value: s.value.clone(),
                    });
                }
                StateLogEntry::TaskCompleted(c) => {
                    self.pending_dispatches.retain(|p| p.task_id != c.task_id);
                    if self.dispatched.remove(&c.task_id) {
                        self.in_flight -= 1;
                    }
                }
                StateLogEntry::Config(_) => {}
            }
        }
        if self.live {
            self.flush_dispatches();
        }
        vec![]
    }

    fn start(&mut self) {
        self.live = true;
        self.flush_dispatches();
    }
}

impl Dispatcher {
    fn flush_dispatches(&mut self) {
        let Some(tx) = &self.tx else { return };

        while self.in_flight < self.max_concurrency {
            let Some(task) = self.pending_dispatches.pop_front() else { break };
            self.in_flight += 1;
            self.dispatched.insert(task.task_id);
            let tx = tx.clone();
            // spawn worker thread with task.step, task.value, tx clone
        }

        if self.pending_dispatches.is_empty() && self.in_flight == 0 {
            self.tx = None;
        }
    }
}
```

### Termination

Worker threads hold `Sender<InFlightResult>` clones. They drop them after sending their result. The Dispatcher drops its sender when `pending_dispatches` is empty and `in_flight` is 0. With all senders dropped, `rx.recv()` returns `Err`, the StateApplier's `recv()` returns `None`, and the coordinator's loop exits.

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

TaskState has two variants. The current `InFlight` variant is replaced by `in_flight: usize` on the Dispatcher. `finally_script` and `retries_remaining` are removed from TaskEntry — the StateApplier looks up the finally script from config when building finally entries, and determines retry exhaustion by counting Retry-origin siblings in the task tree.

## Phasing

Each phase is a separate branch that passes CI and merges independently.

### Phase 0: Data structure cleanup

Independent sub-refactors, each in its own file. Can land in any order.

- **0a.** `EXTRACT_RUN_STATE.md` — **Done.** Moved `tasks` and `next_task_id` into RunState. Also moved `remove_and_notify_parent` onto RunState with deferred parent removal via non-recursive cascade (absorbs 0d).
- **0b.** `REMOVE_INFLIGHT_VARIANT.md` (not yet written) — Replace InFlight TaskState variant with `in_flight: usize` counter on TaskRunner.
- **0c.** `REMOVE_CONFIG_FROM_TASK_ENTRY.md` (not yet written) — Drop `finally_script` and `retries_remaining` from TaskEntry. Look them up from `step_map` when needed.
- **0d.** Absorbed into 0a (EXTRACT_RUN_STATE Phase 2).

### Phase 1: Event loop restructure

**Depends on: None (can run in parallel with Phase 0).**

Convert the Iterator-based loop to an explicit `run()` method with a recv loop. `process_result` still handles everything internally, and the Applier trait isn't introduced yet. The only change is the loop shape: `while let Ok(result) = self.rx.recv() { self.process_result(result); }`.

### Phase 2: Apply pattern

**Depends on: Phase 0a, Phase 1.**

Introduce the `Applier` trait. StateApplier, Dispatcher, and LogApplier all implement it. Build a `Vec<Box<dyn Applier>>`. The coordinator becomes a dumb loop: `recv()` → `apply_all()` → repeat. All logic lives inside the appliers.

### Phase 3: Seeding through apply

**Depends on: Phase 2.**

Restructure `run()` so seed entries go through `apply_all`. `build_seed_entries` produces entries. `apply_all` applies them through all appliers.

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

After: The StateApplier produces entries from `recv()`. Those entries flow through `apply_all()`, which feeds them to every applier — StateApplier updates its task tree, LogApplier writes to disk, Dispatcher queues/dequeues. All see the same entries.

```rust
// StateApplier::recv() produces entries from InFlightResult:
vec![
    StateLogEntry::TaskCompleted(TaskCompleted { task_id, outcome: ... }),
    StateLogEntry::TaskSubmitted(TaskSubmitted { task_id: child_id, ... }),
    // ...
]

// apply_all feeds them through every applier:
// StateApplier::apply() updates task tree, detects removed parents, emits finally entries
// LogApplier::apply() writes entries to disk
// Dispatcher::apply() queues/dequeues tasks
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

After: The StateApplier detects removed parents during `apply()`, looks up finally scripts from config, allocates IDs, and emits `TaskSubmitted` entries. Those entries flow through `apply_all` back to all appliers — the StateApplier inserts the task into its tree, the Dispatcher queues it, the LogApplier writes it. No config on TaskEntry.

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

After: the coordinator is a dumb loop over trait methods.

```rust
loop {
    let mut entries = Vec::new();
    for applier in appliers.iter_mut() {
        if let Some(new) = applier.recv() {
            entries.extend(new);
        }
    }
    if entries.is_empty() { break; }
    apply_all(&mut appliers, &entries);
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

`finally_script` and `retries_remaining` are looked up from config when needed.

## Testing

```rust
// RunState (dependency tracker, no I/O)
#[test] fn apply_submitted_creates_pending_entry()
#[test] fn apply_submitted_advances_next_task_id()
#[test] fn apply_completed_success_no_children_removes_task()
#[test] fn apply_completed_success_with_children_transitions_to_waiting()
#[test] fn apply_completed_child_decrements_parent_count()
#[test] fn apply_completed_last_child_removes_parent()
#[test] fn apply_completed_last_child_captures_removed_parent()
#[test] fn apply_completed_recursive_removal_up_tree()
#[test] fn apply_completed_failed_removes_task()

// StateApplier
#[test] fn recv_produces_entries_from_result()
#[test] fn recv_returns_none_before_start()
#[test] fn recv_returns_none_when_channel_closed()
#[test] fn apply_produces_finally_entries_for_removed_parents()
#[test] fn apply_skips_finally_when_no_script()
#[test] fn apply_allocates_ids_for_finally_entries()
#[test] fn finally_entries_loop_until_stable()

// LogApplier
#[test] fn writes_all_entry_variants()
#[test] fn skips_writes_before_start()

// Dispatcher
#[test] fn apply_queues_submitted_tasks()
#[test] fn apply_dequeues_completed_tasks()
#[test] fn apply_does_not_flush_before_start()
#[test] fn start_enables_flushing()
#[test] fn flush_dispatches_up_to_max_concurrency()
#[test] fn flush_drops_tx_when_empty_and_no_in_flight()
#[test] fn completed_only_decrements_in_flight_for_dispatched_tasks()

// apply_all
#[test] fn apply_all_feeds_entries_through_all_appliers()
#[test] fn apply_all_loops_on_produced_entries()
#[test] fn apply_all_terminates_when_no_new_entries()

// Resume
#[test] fn replay_log_reconstructs_identical_state()
#[test] fn dispatcher_only_dispatches_incomplete_tasks_after_start()
#[test] fn log_applier_does_not_write_during_replay()
```
