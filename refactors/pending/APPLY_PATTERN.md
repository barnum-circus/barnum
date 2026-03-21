# Apply Pattern for State/Log Consistency

**Status:** Not started

**Depends on:** None

## Motivation

Currently, state changes and log writes are separate operations scattered throughout `TaskRunner`:

```rust
// BEFORE: Two operations that must stay in sync — but nothing enforces it
self.state_log.write(TaskSubmitted { ... });
self.tasks.insert(id, entry);
```

This creates risk of:
1. Forgetting to write to log when changing state
2. Log/state getting out of sync
3. Bugs where state is updated but log isn't (or vice versa)
4. Resume logic diverging from live logic

## Primitives

### StateLogEntry

Everything in the system is a `StateLogEntry`. The log is the complete record — it captures both facts (what agents returned) and interpretations (what we decided to do about it).

```rust
enum StateLogEntry {
    Config(StateLogConfig),
    TaskSubmitted(TaskSubmitted),
    TaskCompleted(TaskCompleted),
    /// Raw agent response. Logged for completeness and auditability.
    /// RunState ignores this variant — the derived events that follow
    /// (TaskCompleted, TaskSubmitted) handle state changes.
    Completion(CompletionData),
}

struct CompletionData {
    task_id: LogTaskId,
    response: String,  // raw agent output
}
```

No wrapper type. No `Event` enum. One currency everywhere.

### Applier

Reacts to log entries. Optionally produces follow-up entries that cascade through all appliers.

```rust
trait Applier {
    /// React to a log entry. Returns follow-up entries that will cascade
    /// through all appliers until quiescence.
    fn apply(&mut self, entry: &StateLogEntry) -> Vec<StateLogEntry>;
}
```

One method. That's the whole trait.

### EventLoop

Drives the cascade: an entry produces follow-up entries, which produce more entries, until quiescence. FIFO ordering ensures entries are processed in the order they were produced.

```rust
struct EventLoop {
    appliers: Vec<Box<dyn Applier>>,
    rx: Receiver<CompletionData>,
}

impl EventLoop {
    /// Cascade an entry through all appliers until quiescence.
    fn apply_all(&mut self, entry: StateLogEntry) {
        let mut queue = VecDeque::from([entry]);
        while let Some(e) = queue.pop_front() {
            for applier in &mut self.appliers {
                queue.extend(applier.apply(&e));
            }
        }
    }

    /// Receive completions and cascade them until the run is done.
    fn run(&mut self) {
        while let Ok(completion) = self.rx.recv() {
            self.apply_all(StateLogEntry::Completion(completion));
        }
    }
}
```

The channel carries `CompletionData` (not `StateLogEntry` — worker threads only produce completions). The EventLoop wraps it in `StateLogEntry::Completion` before cascading.

Termination: when the Dispatcher has no pending tasks and `in_flight == 0`, it drops its `Sender`. Worker threads hold `Sender` clones — as the last one finishes and drops its clone, all senders are gone, `rx.recv()` returns `Err`, and the loop exits.

**Example cascade** for a completion that spawns children:

1. `StateLogEntry::Completion(raw)` enters cascade
2. Dispatcher returns `[TaskCompleted, TaskSubmitted(child1), TaskSubmitted(child2)]`
3. LogApplier writes `Completion` to disk, returns `[]`
4. Queue now: `[TaskCompleted, TaskSubmitted(child1), TaskSubmitted(child2)]`
5. `TaskCompleted` processed: Dispatcher updates RunState, LogApplier writes to disk
6. `TaskSubmitted(child1)` processed: Dispatcher adds to RunState and dispatches, LogApplier writes
7. `TaskSubmitted(child2)` processed: same
8. Queue empty — quiescence

Log on disk after this cascade:
```
Completion { task_id: 0, response: "[{\"kind\": \"Child\", ...}]" }
TaskCompleted { task_id: 0, outcome: Success { spawned: [1, 2] } }
TaskSubmitted { task_id: 1, step: "Child", ... }
TaskSubmitted { task_id: 2, step: "Child", ... }
```

During replay, RunState ignores the `Completion` line and processes the rest.

## Appliers

### Dispatcher

Owns `RunState` plus all runtime machinery. Dispatches pending tasks in its constructor — no `start()` method, no mode transitions. By the time the event loop calls `run()`, tasks are already in flight.

```rust
struct Dispatcher {
    state: RunState,
    config: Config,
    schemas: CompiledSchemas,
    step_map: HashMap<StepName, Step>,
    pool: PoolConnection,
    tx: Option<Sender<CompletionData>>,
    in_flight: usize,
    max_concurrency: usize,
}

impl Dispatcher {
    fn new(
        state: RunState,
        config: Config,
        schemas: CompiledSchemas,
        pool: PoolConnection,
        tx: Sender<CompletionData>,
        max_concurrency: usize,
    ) -> Self {
        let step_map = config.steps.iter()
            .map(|s| (s.name.clone(), s.clone()))
            .collect();
        let mut d = Dispatcher {
            state, config, schemas, step_map, pool,
            tx: Some(tx), in_flight: 0, max_concurrency,
        };
        d.dispatch_pending();
        d
    }
}

impl Applier for Dispatcher {
    fn apply(&mut self, entry: &StateLogEntry) -> Vec<StateLogEntry> {
        match entry {
            StateLogEntry::Completion(raw) => {
                self.in_flight -= 1;
                self.build_completion_events(raw)
            }
            _ => {
                self.state.apply(entry);
                let events = self.drain_ready_parents();
                self.dispatch_pending();
                events
            }
        }
    }
}
```

For `Completion`: the Dispatcher decrements `in_flight` and interprets the raw response — parsing JSON, validating against schemas, deciding success/failure/retry. Returns the derived events (`TaskCompleted`, `TaskSubmitted` for children/retries/finally). These cascade through all appliers.

For everything else: the Dispatcher forwards to RunState, drains any parents ready for finally, and dispatches pending tasks.

#### Dispatching

```rust
impl Dispatcher {
    fn dispatch_pending(&mut self) {
        let Some(tx) = &self.tx else { return };

        while self.in_flight < self.max_concurrency {
            let Some(task_id) = self.state.next_pending() else { break };
            let entry = self.state.tasks.get(&task_id).expect("task must exist");
            let value = match &entry.state {
                TaskState::Pending { value } => value.clone(),
                _ => unreachable!("next_pending returned non-Pending task"),
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

#### Finally handling

When a `TaskCompleted` for the last child cascades through `apply`, RunState transitions the parent to `ChildrenComplete` and pushes its ID onto `ready_for_finally`. The Dispatcher drains this queue:

```rust
impl Dispatcher {
    fn drain_ready_parents(&mut self) -> Vec<StateLogEntry> {
        let mut events = Vec::new();
        for parent_id in self.state.drain_ready_for_finally() {
            let entry = self.state.tasks.get(&parent_id).expect("parent must exist");
            let TaskState::ChildrenComplete { finally_value } = &entry.state else {
                unreachable!("ready_for_finally contained non-ChildrenComplete task");
            };
            let finally_script = self.step_map[&entry.step].finally.as_ref();
            if let Some(script) = finally_script {
                let id = self.state.next_id();
                events.push(StateLogEntry::TaskSubmitted(TaskSubmitted {
                    task_id: id,
                    step: script.step.clone(),
                    value: finally_value.clone(),
                    parent_id: Some(parent_id),
                    origin: TaskOrigin::Finally,
                }));
            }
            self.state.tasks.remove(&parent_id);
        }
        events
    }
}
```

The returned `TaskSubmitted` events cascade through all appliers — LogApplier writes them, Dispatcher's `apply` adds them to RunState and dispatches.

### LogApplier

Writes every entry to the NDJSON state log. Including `Completion` entries — the log is the complete record. Zero logic.

```rust
struct LogApplier {
    writer: io::BufWriter<File>,
}

impl LogApplier {
    fn new(path: &Path) -> io::Result<Self> {
        let file = File::create(path)?;
        Ok(LogApplier { writer: io::BufWriter::new(file) })
    }

    /// For resume: copy old log to new file, then append.
    fn from_existing(old_log: &Path, new_path: &Path) -> io::Result<Self> {
        std::fs::copy(old_log, new_path)?;
        let file = OpenOptions::new().append(true).open(new_path)?;
        Ok(LogApplier { writer: io::BufWriter::new(file) })
    }
}

impl Applier for LogApplier {
    fn apply(&mut self, entry: &StateLogEntry) -> Vec<StateLogEntry> {
        barnum_state::write_entry(&mut self.writer, entry)
            .expect("failed to write state log entry");
        vec![]
    }
}
```

## State

### RunState

Pure state machine owned by the Dispatcher. No I/O, no side effects. Ignores `Completion` entries — those are interpreted by the Dispatcher, not the state machine.

```rust
struct RunState {
    /// All active tasks. Removed when fully complete.
    /// BTreeMap ordering = FIFO dispatch order (task IDs are monotonic).
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    /// Monotonic counter for assigning task IDs.
    next_task_id: u32,
    /// Parents whose children all completed, ready for finally.
    /// Drained by Dispatcher after each apply.
    ready_for_finally: Vec<LogTaskId>,
}

impl RunState {
    fn next_id(&mut self) -> LogTaskId {
        let id = LogTaskId(self.next_task_id);
        self.next_task_id += 1;
        id
    }

    fn apply(&mut self, entry: &StateLogEntry) {
        match entry {
            StateLogEntry::Config(_) | StateLogEntry::Completion(_) => {}
            StateLogEntry::TaskSubmitted(s) => self.apply_submitted(s),
            StateLogEntry::TaskCompleted(c) => self.apply_completed(c),
        }
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
            TaskOutcome::Failed(_) => {
                self.remove_and_notify_parent(completed.task_id);
            }
        }
    }

    fn remove_and_notify_parent(&mut self, task_id: LogTaskId) {
        let parent_id = self.tasks.get(&task_id).and_then(|e| e.parent_id);
        self.tasks.remove(&task_id);

        if let Some(pid) = parent_id {
            if let Some(parent) = self.tasks.get_mut(&pid) {
                if let TaskState::WaitingForChildren {
                    pending_children_count, finally_value
                } = &mut parent.state {
                    match NonZeroU16::new(pending_children_count.get() - 1) {
                        Some(n) => *pending_children_count = n,
                        None => {
                            let fv = finally_value.clone();
                            parent.state = TaskState::ChildrenComplete {
                                finally_value: fv,
                            };
                            self.ready_for_finally.push(pid);
                        }
                    }
                }
            }
        }
    }

    fn drain_ready_for_finally(&mut self) -> Vec<LogTaskId> {
        std::mem::take(&mut self.ready_for_finally)
    }

    fn next_pending(&self) -> Option<LogTaskId> {
        self.tasks.iter()
            .find(|(_, e)| matches!(e.state, TaskState::Pending { .. }))
            .map(|(id, _)| *id)
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
    /// Waiting to be dispatched (or dispatched, waiting for completion).
    Pending { value: StepInputValue },
    /// Children are running. Count is always non-zero.
    WaitingForChildren {
        pending_children_count: NonZeroU16,
        finally_value: StepInputValue,
    },
    /// All children complete. Dispatcher will submit the finally task
    /// and then remove this entry.
    ChildrenComplete { finally_value: StepInputValue },
}
```

Compared to the current code:

- **`InFlight` removed.** Dispatch is transient — tracked by `Dispatcher.in_flight`. RunState sees dispatched tasks as `Pending` until `TaskCompleted` arrives. On resume, pending-but-dispatched tasks get re-dispatched. Correct by design.
- **`ChildrenComplete` added.** Explicit state for "children done, finally pending." Previously the parent was removed immediately, losing the finally context.
- **`finally_script`, `retries_remaining` removed.** Config concerns, looked up from `step_map` at dispatch time.

## Usage

Seeding is construction. Build RunState and write initial log entries before the EventLoop exists. The Dispatcher constructor dispatches pending tasks. The event loop just receives completions and cascades them.

### Fresh run

```rust
pub fn run(config: Config, initial_tasks: Vec<Task>, runner_config: &RunnerConfig) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();

    let mut state = RunState::new();
    let mut log_applier = LogApplier::new(&runner_config.state_log_path)?;

    // Seed: build state and write log entries.
    let config_entry = StateLogEntry::Config(StateLogConfig {
        config: serde_json::to_value(&config)?,
    });
    state.apply(&config_entry);
    log_applier.apply(&config_entry);

    for task in initial_tasks {
        let id = state.next_id();
        let entry = StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: id,
            step: task.step,
            value: task.value,
            parent_id: None,
            origin: TaskOrigin::Initial,
        });
        state.apply(&entry);
        log_applier.apply(&entry);
    }

    // Dispatcher dispatches pending tasks in constructor.
    let schemas = CompiledSchemas::compile(&config)?;
    let pool = PoolConnection::connect(&runner_config.troupe_root)?;
    let dispatcher = Dispatcher::new(state, config, schemas, pool, tx, runner_config.max_concurrency);

    let mut event_loop = EventLoop {
        appliers: vec![Box::new(dispatcher), Box::new(log_applier)],
        rx,
    };

    event_loop.run();
    Ok(())
}
```

### Resume

```rust
pub fn resume(old_log: &Path, runner_config: &RunnerConfig) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    let config = extract_config_from_log(old_log)?;

    // Rebuild RunState from the old log.
    // Completion entries are no-ops — the derived events handle state.
    let mut state = RunState::new();
    for entry in barnum_state::read_entries(File::open(old_log)?) {
        state.apply(&entry);
    }

    // Copy old log to new file, then append new entries.
    let log_applier = LogApplier::from_existing(old_log, &runner_config.state_log_path)?;

    // Dispatcher dispatches pending tasks (those that were in-flight when the old run stopped).
    let schemas = CompiledSchemas::compile(&config)?;
    let pool = PoolConnection::connect(&runner_config.troupe_root)?;
    let dispatcher = Dispatcher::new(state, config, schemas, pool, tx, runner_config.max_concurrency);

    let mut event_loop = EventLoop {
        appliers: vec![Box::new(dispatcher), Box::new(log_applier)],
        rx,
    };

    event_loop.run();
    Ok(())
}
```

Same shape: build state → construct dispatcher → run. Fresh run seeds from arguments. Resume seeds from a log file.

## Before/After Summary

| Before (`TaskRunner`) | After |
|---|---|
| One monolith struct with 12 fields | `EventLoop` + `Dispatcher` + `LogApplier` + `RunState` |
| `queue_task` manually writes log AND updates state | `apply_all` cascades through all appliers uniformly |
| `task_succeeded`/`task_failed` manually writes log AND updates state | `build_completion_events` returns entries; cascade handles the rest |
| `tx: Sender` (always valid) | `tx: Option<Sender>` (None = terminated, triggers loop exit) |
| `InFlight` variant in `TaskState` (transient state in persistent model) | `in_flight: usize` on Dispatcher only |
| `finally_script` on `TaskEntry` (config in state) | Looked up from `step_map` at dispatch time |
| Resume requires separate code path | Same build → construct → run shape, different seed source |
| Raw completions not logged | `Completion` variant in `StateLogEntry` — log is complete |

## Testing

```rust
// RunState in isolation (pure state machine, no I/O)
#[test] fn apply_submitted_creates_pending_entry()
#[test] fn apply_submitted_advances_next_task_id()
#[test] fn apply_completion_is_no_op()
#[test] fn apply_completed_success_no_children_removes_task()
#[test] fn apply_completed_success_with_children_transitions_to_waiting()
#[test] fn apply_completed_child_decrements_parent_count()
#[test] fn apply_completed_last_child_transitions_to_children_complete()
#[test] fn apply_completed_last_child_pushes_to_ready_for_finally()
#[test] fn apply_completed_failed_removes_task()

// LogApplier in isolation
#[test] fn writes_all_entry_variants_including_completion()
#[test] fn from_existing_copies_old_log_then_appends()

// Dispatcher in isolation (with mock tx/rx)
#[test] fn constructor_dispatches_pending_tasks()
#[test] fn constructor_with_empty_state_drops_tx()
#[test] fn apply_completion_returns_task_completed_events()
#[test] fn apply_completion_returns_child_submitted_events()
#[test] fn apply_completion_returns_retry_submitted_events()
#[test] fn dispatches_up_to_max_concurrency()
#[test] fn drops_tx_when_empty_and_no_in_flight()
#[test] fn drain_ready_parents_submits_finally_tasks()

// EventLoop: cascade behavior
#[test] fn apply_all_cascades_until_quiescence()
#[test] fn apply_all_processes_entries_in_fifo_order()

// Integration: replay
#[test] fn replay_reconstructs_identical_run_state()
#[test] fn replay_copies_entries_to_new_log()
#[test] fn resumed_run_dispatches_incomplete_tasks()
```

## Migration Path

1. Add `Completion` variant to `StateLogEntry` in `barnum_state`
2. Define `Applier` trait
3. Extract `RunState` from `TaskRunner` (pure state machine)
4. Add `ChildrenComplete` to `TaskState`, `ready_for_finally` to `RunState`
5. Build `Dispatcher` (owns RunState, dispatches in constructor, implements `Applier`)
6. Build `LogApplier` with `from_existing` (implements `Applier`)
7. Build `EventLoop` (owns applier vec + rx, drives the cascade)
8. Rewrite `run()` and `resume()` as: build state → construct → run
9. Delete old `TaskRunner`
10. Remove `InFlight` from `TaskState`, config fields from `TaskEntry`
11. Verify all tests pass
