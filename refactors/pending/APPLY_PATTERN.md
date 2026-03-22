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

Barnum's current loop (`crates/barnum_config/src/runner/mod.rs:951`):

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

Every applier sees task IDs only through `StateLogEntry` values. Appliers read IDs from the entries they receive; they never allocate IDs independently. RunState owns the ID counter (`next_task_id: u32`) and advances it past any ID it sees in an entry via `max(current, entry.task_id + 1)`. Only the Engine allocates new IDs — for children, retries, and finally tasks.

Workers produce only `TaskCompleted` entries, which reference an existing task ID (no allocation needed). The Engine processes completions, allocates IDs for any resulting children/retries, and produces `TaskSubmitted` entries.

### Target event loop

The coordinator owns a `Receiver<StateLogEntry>` and a `Vec<Box<dyn Applier>>`. It receives a single entry from the channel, wraps it in a slice, and passes it to every applier via `process_entries`. That's it — the coordinator has no knowledge of RunState, config, or any other internal detail.

```rust
pub fn run(
    config: &Config,
    initial_tasks: Vec<Task>,
    runner_config: &RunnerConfig,
) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();

    let mut appliers: Vec<Box<dyn Applier>> = vec![
        Box::new(Engine::new(config, RunState::new(), runner_config, tx)),
        Box::new(LogApplier::new(&runner_config.state_log_path)?),
    ];

    // Seed
    let seed = build_seed_entries(config, &initial_tasks);
    process_entries(&mut appliers, &seed);

    // Event loop (follows Troupe pattern: receive → apply)
    while let Ok(entry) = rx.recv() {
        process_entries(&mut appliers, &[entry]);
    }

    Ok(())
}

pub fn resume(old_log_path: &Path, runner_config: &RunnerConfig) -> io::Result<()> {
    let (config, run_state) = replay_log(old_log_path)?;
    let (tx, rx) = mpsc::channel();

    let mut appliers: Vec<Box<dyn Applier>> = vec![
        Box::new(Engine::new(&config, run_state, runner_config, tx)),
        Box::new(LogApplier::new(&runner_config.state_log_path)?),
    ];

    // Kick — flushes pending dispatches from replay
    process_entries(&mut appliers, &[]);

    // Event loop
    while let Ok(entry) = rx.recv() {
        process_entries(&mut appliers, &[entry]);
    }

    Ok(())
}

fn process_entries(appliers: &mut [Box<dyn Applier>], entries: &[StateLogEntry]) {
    for applier in appliers {
        applier.apply(entries);
    }
}
```

`tx` moves into the Engine — no clone, no coordinator-side sender. Workers hold `tx` clones (given by the Engine when it spawns them). The Engine sends each produced entry individually on `tx`. When all senders are dropped, `rx.recv()` returns `Err` and the loop exits.

The current loop already has the same shape: receive, process. The refactor converts it in two stages:

1. **Event loop restructure** (Phase 1): Convert the Iterator to an explicit recv loop where `process_result` still handles everything internally.

2. **Apply pattern** (Phase 2): Introduce the Applier trait. Engine and LogApplier implement it. The coordinator becomes the loop above.

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
    spawned: Vec<TaskSpec>,
    finally_value: StepInputValue,
}

struct TaskFailed {
    reason: FailureReason,
    retry: Option<TaskSpec>,
}

struct TaskSpec {
    step: StepName,
    value: StepInputValue,
}
```

`TaskSuccess` and `TaskFailed` carry task specs, not allocated IDs. IDs don't exist yet when the worker produces the entry — the Engine allocates them when processing the completion. The subsequent `TaskSubmitted` entries (produced by the Engine) carry the allocated IDs.

Each variant records a fact. The Engine derives task removal internally when all children of a parent complete.

## Applier

```rust
trait Applier {
    fn apply(&mut self, entries: &[StateLogEntry]);
}
```

The coordinator calls `process_entries` which passes entries to `apply()` on each applier. No other methods on the trait.

### Engine

Owns the full execution lifecycle: task state, dispatch, and entry production. Holds a `Sender<StateLogEntry>` to feed entries back to the coordinator channel.

```rust
struct Engine<'a> {
    state: RunState,
    config: &'a Config,
    tx: Option<Sender<StateLogEntry>>,
    pool: PoolConnection,
    in_flight: usize,
    max_concurrency: usize,
    pending_dispatches: VecDeque<PendingTask>,
    dispatched: HashSet<LogTaskId>,
}

struct RunState {
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    next_task_id: u32,
    removed_parents: Vec<RemovedParent>,
}

struct RemovedParent {
    task_id: LogTaskId,
    step: StepName,
    parent_id: Option<LogTaskId>,
    finally_value: StepInputValue,
}

struct PendingTask {
    task_id: LogTaskId,
    step: StepName,
    value: StepInputValue,
}
```

**`apply()`**: Updates state, produces entries for children/retries/finally, sends them on `tx`, and flushes dispatches.

```rust
fn apply(&mut self, entries: &[StateLogEntry]) {
    let mut produced = Vec::new();

    for entry in entries {
        match entry {
            StateLogEntry::TaskSubmitted(s) => {
                if self.state.tasks.contains_key(&s.task_id) {
                    continue; // already applied (feedback from own production)
                }
                self.state.apply_submitted(s);
                self.pending_dispatches.push_back(PendingTask {
                    task_id: s.task_id,
                    step: s.step.clone(),
                    value: s.value.clone(),
                });
            }
            StateLogEntry::TaskCompleted(c) => {
                produced.extend(self.process_completion(c));
                self.pending_dispatches.retain(|p| p.task_id != c.task_id);
                if self.dispatched.remove(&c.task_id) {
                    self.in_flight -= 1;
                }
            }
            StateLogEntry::Config(_) => {}
        }
    }

    // Finally cascade
    produced.extend(self.process_removed_parents());

    // Send produced entries and flush
    if let Some(tx) = &self.tx {
        for entry in produced {
            tx.send(entry).expect("[P031] channel open");
        }
    }

    self.flush_dispatches();
}
```

The `contains_key` check in `TaskSubmitted` prevents double-processing when the Engine's own entries loop back through the channel. During replay (which bypasses the Engine entirely), this never triggers.

**`process_completion()`**: Handles the result interpretation that today lives in `process_result`. Captures the task's parent_id before calling `apply_completed` (which may remove the task), then allocates IDs and produces `TaskSubmitted` entries for children/retries. Applies them internally.

```rust
fn process_completion(&mut self, completed: &TaskCompleted) -> Vec<StateLogEntry> {
    let parent_id = self.state.tasks.get(&completed.task_id)
        .and_then(|e| e.parent_id);

    self.state.apply_completed(completed);

    match &completed.outcome {
        TaskOutcome::Success(success) if !success.spawned.is_empty() => {
            success.spawned.iter().map(|spec| {
                let id = self.state.next_id();
                let submitted = TaskSubmitted {
                    task_id: id,
                    step: spec.step.clone(),
                    value: spec.value.clone(),
                    parent_id: Some(completed.task_id),
                    origin: TaskOrigin::Spawned,
                };
                self.state.apply_submitted(&submitted);
                self.pending_dispatches.push_back(PendingTask {
                    task_id: id, step: spec.step.clone(), value: spec.value.clone(),
                });
                StateLogEntry::TaskSubmitted(submitted)
            }).collect()
        }
        TaskOutcome::Failed(failed) if failed.retry.is_some() => {
            let spec = failed.retry.as_ref().unwrap();
            let id = self.state.next_id();
            let submitted = TaskSubmitted {
                task_id: id,
                step: spec.step.clone(),
                value: spec.value.clone(),
                parent_id,
                origin: TaskOrigin::Retry { replaces: completed.task_id },
            };
            self.state.apply_submitted(&submitted);
            self.pending_dispatches.push_back(PendingTask {
                task_id: id, step: spec.step.clone(), value: spec.value.clone(),
            });
            vec![StateLogEntry::TaskSubmitted(submitted)]
        }
        _ => vec![],
    }
}
```

**`process_removed_parents()`**: Runs the finally cascade. For each removed parent: look up the finally script from config, allocate an ID, apply the entry internally (which increments the grandparent's child count), then remove the parent from the map.

```rust
fn process_removed_parents(&mut self) -> Vec<StateLogEntry> {
    let mut entries = Vec::new();
    while let Some(removed) = self.state.removed_parents.pop() {
        let script = self.config.step_map.get(&removed.step)
            .and_then(|s| s.finally.as_ref());
        if let Some(script) = script {
            let id = self.state.next_id();
            let submitted = TaskSubmitted {
                task_id: id,
                step: script.step.clone(),
                value: removed.finally_value,
                parent_id: removed.parent_id,
                origin: TaskOrigin::Finally { finally_for: removed.task_id },
            };
            self.state.apply_submitted(&submitted);
            self.pending_dispatches.push_back(PendingTask {
                task_id: id, step: script.step.clone(), value: submitted.value.clone(),
            });
            entries.push(StateLogEntry::TaskSubmitted(submitted));
        }
        self.state.remove_and_notify_parent(removed.task_id);
    }
    entries
}
```

The ordering — apply finally entry then remove parent — ensures the grandparent's child count is incremented before it could hit zero from the parent's removal. This matches the current `schedule_removed_finally` logic.

**`flush_dispatches()`**: Spawns worker threads. Each worker gets a `tx` clone and the step config.

```rust
fn flush_dispatches(&mut self) {
    let Some(tx) = &self.tx else { return };

    while self.in_flight < self.max_concurrency {
        let Some(task) = self.pending_dispatches.pop_front() else { break };
        self.in_flight += 1;
        self.dispatched.insert(task.task_id);
        let tx = tx.clone();
        // spawn worker thread with task, step config, tx
    }

    if self.pending_dispatches.is_empty() && self.in_flight == 0 {
        self.tx = None; // drop sender → channel closes when workers finish
    }
}
```

**Workers**: Each worker thread:

1. Runs the task via the pool
2. Interprets the result (`process_submit_result`, post hooks) — step config captured in closure
3. Produces a single `TaskCompleted` entry (no ID allocation)
4. Sends on `tx`, drops `tx` clone

**RunState internals**:

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
        // Finally tasks increment their parent's child count
        if matches!(submitted.origin, TaskOrigin::Finally { .. }) {
            if let Some(parent_id) = submitted.parent_id {
                self.increment_pending_children(parent_id);
            }
        }
    }

    fn apply_completed(&mut self, completed: &TaskCompleted) {
        match &completed.outcome {
            TaskOutcome::Success(success) => {
                if success.spawned.is_empty() {
                    self.remove_and_notify_parent(completed.task_id);
                } else {
                    let count = success.spawned.len();
                    let entry = self.tasks.get_mut(&completed.task_id)
                        .expect("[P033] completed task must exist");
                    entry.state = TaskState::WaitingForChildren {
                        pending_children_count: NonZeroU16::new(count as u16)
                            .expect("[P034] spawned is non-empty"),
                        finally_value: success.finally_value.clone(),
                    };
                }
            }
            TaskOutcome::Failed(failed) => {
                if failed.retry.is_some() {
                    self.tasks.remove(&completed.task_id);
                } else {
                    self.remove_and_notify_parent(completed.task_id);
                }
            }
        }
    }
}
```

`remove_and_notify_parent` is unchanged from EXTRACT_RUN_STATE (non-recursive, accumulates into `removed_parents`).

### LogApplier

```rust
struct LogApplier {
    writer: io::BufWriter<File>,
}

impl Applier for LogApplier {
    fn apply(&mut self, entries: &[StateLogEntry]) {
        for entry in entries {
            barnum_state::write_entry(&mut self.writer, entry)
                .expect("[P032] failed to write state log entry");
        }
    }
}
```

Created after replay so it never writes replayed entries. Writes every entry it receives.

### Termination

Worker threads hold `Sender` clones. They drop them after sending results. The Engine drops its `tx` when `pending_dispatches` is empty and `in_flight == 0`. With all senders dropped, `rx.recv()` returns `Err` and the coordinator loop exits.

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

TaskState has two variants. The current `InFlight` variant is replaced by `in_flight: usize` + `dispatched: HashSet<LogTaskId>` on the Engine. `finally_script` and `retries_remaining` are removed from TaskEntry — the Engine looks up the finally script from config when building finally entries, and workers determine retry exhaustion from step options.

## Replay

Replay runs before the event loop and constructs RunState directly, bypassing the Applier trait:

```rust
fn replay_log(path: &Path) -> io::Result<(Config, RunState)> {
    let entries = barnum_state::read_entries(path);
    let mut state = RunState::new();
    let mut config_json = None;
    for entry in entries {
        match &entry {
            StateLogEntry::Config(c) => config_json = Some(c.config.clone()),
            StateLogEntry::TaskSubmitted(s) => state.apply_submitted(s),
            StateLogEntry::TaskCompleted(c) => state.apply_completed(c),
        }
    }
    let config = /* deserialize config_json */;
    Ok((config, state))
}
```

After replay:
- `RunState` contains only active tasks (pending + waiting-for-children)
- `next_task_id` is past all replayed IDs (advanced by `apply_submitted`)
- `Engine::new()` initializes `pending_dispatches` from RunState's pending tasks
- `LogApplier` is created at the current log position (appending)
- The kick (`process_entries(&mut appliers, &[])`) flushes dispatches for remaining pending tasks

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

**Depends on: Phase 0, Phase 1.**

Introduce the `Applier` trait. Engine and LogApplier implement it. Build a `Vec<Box<dyn Applier>>`. The coordinator becomes the event loop described above.

### Phase 3: Seeding through apply

**Depends on: Phase 2.**

Restructure `run()` so seed entries go through `process_entries` directly (not through the channel). `build_seed_entries` produces entries, `process_entries` feeds them through all appliers.

## Before/After

### Task success: log and state are separate operations

Before (`runner/mod.rs:761`): `task_succeeded` manually writes the log, then separately mutates state. Miss either one and they diverge.

```rust
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
    self.write_log(&StateLogEntry::TaskCompleted(...));  // 1. write log
    if let Some(hook) = finally_hook {
        self.schedule_finally(task_id, hook, value);     // 2. mutate state (finally)
    }
    self.remove_and_notify_parent(task_id);              // 3. mutate state (remove)
}
```

After: Workers produce `TaskCompleted` entries. Those entries flow through all appliers — Engine allocates IDs for children and sends `TaskSubmitted` entries on tx, LogApplier writes to disk. State and log see the same entries and can never diverge.

### Main loop: scattered responsibilities

Before (`runner/mod.rs:951`): Iterator trait with dispatch and processing mixed together.

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

After: the coordinator is a dumb loop.

```rust
while let Ok(entry) = rx.recv() {
    process_entries(&mut appliers, &[entry]);
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
// RunState
#[test] fn apply_submitted_creates_pending_entry()
#[test] fn apply_submitted_advances_next_task_id()
#[test] fn apply_submitted_increments_parent_count_for_finally()
#[test] fn apply_completed_success_no_children_removes_task()
#[test] fn apply_completed_success_with_children_transitions_to_waiting()
#[test] fn apply_completed_child_decrements_parent_count()
#[test] fn apply_completed_last_child_captures_removed_parent()
#[test] fn apply_completed_recursive_removal_up_tree()
#[test] fn apply_completed_failed_removes_task()

// Engine
#[test] fn apply_updates_state_and_queues_dispatches()
#[test] fn apply_dequeues_completed_tasks()
#[test] fn apply_skips_already_known_tasks()
#[test] fn process_completion_allocates_child_ids()
#[test] fn process_completion_allocates_retry_id()
#[test] fn process_completion_preserves_parent_id_for_retry()
#[test] fn flush_dispatches_up_to_max_concurrency()
#[test] fn flush_drops_tx_when_empty_and_no_in_flight()
#[test] fn completed_only_decrements_in_flight_for_dispatched_tasks()
#[test] fn process_removed_parents_produces_finally_entries()
#[test] fn process_removed_parents_increments_grandparent_before_removal()
#[test] fn finally_cascade_handles_multi_level_tree()

// LogApplier
#[test] fn writes_all_entry_variants()

// Coordinator
#[test] fn process_entries_feeds_all_appliers()
#[test] fn event_loop_exits_when_channel_closes()

// Replay
#[test] fn replay_reconstructs_state_from_log()
#[test] fn replay_advances_next_task_id()
#[test] fn engine_dispatches_remaining_tasks_after_replay()

// Workers
#[test] fn worker_produces_completion_entry()
#[test] fn worker_determines_success_with_spawned_specs()
#[test] fn worker_determines_retry_with_spec()
```
