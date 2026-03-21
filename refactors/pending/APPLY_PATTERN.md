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

## Architecture

### The event loop

The outermost structure is an event loop with a `Vec<Box<dyn Applier>>`. Every state event goes through every applier. The event loop doesn't know or care what the appliers do internally. No applier is special. No downcasting.

```rust
struct EventLoop {
    appliers: Vec<Box<dyn Applier>>,
    rx: Receiver<RawCompletion>,
}

impl EventLoop {
    /// Feed a state event through all appliers.
    fn apply_all(&mut self, entry: StateLogEntry) {
        for applier in &mut self.appliers {
            applier.apply(&entry);
        }
    }

    /// Main execution loop.
    fn run(&mut self) {
        loop {
            let Ok(raw) = self.rx.recv() else { break };

            // Phase 1: let appliers convert the raw completion into state events.
            let mut events = Vec::new();
            for applier in &mut self.appliers {
                events.extend(applier.process_completion(&raw));
            }

            // Phase 2: feed those events through all appliers.
            for event in events {
                self.apply_all(event);
            }
        }
    }
}
```

Termination: when the Dispatcher has no pending tasks and `in_flight` hits 0, it drops its `tx`. In-flight worker threads hold their own `tx` clones — as the last one sends its result and drops its clone, all senders are gone, `rx.recv()` returns `Err`, loop exits.

### The Applier trait

```rust
trait Applier {
    /// React to a state event. Every applier sees every event.
    fn apply(&mut self, entry: &StateLogEntry);

    /// Process a raw task completion from a worker thread.
    /// Returns state events that should be fed through all appliers.
    /// Default: no-op (most appliers don't produce events).
    fn process_completion(&mut self, _result: &RawCompletion) -> Vec<StateLogEntry> {
        vec![]
    }
}
```

Two methods. `apply()` is for reacting to state events. `process_completion()` is for converting raw task results into state events. Only the Dispatcher implements `process_completion()` — all others use the default empty impl.

### Where state lives

Every applier owns its own state. No shared state. No multiple owners.

| Applier | Owns | Persisted? | Survives resume? |
|---------|------|------------|------------------|
| **Dispatcher** | `RunState` (task map, next_task_id), pool connection, `tx`, `in_flight`, config, schemas | RunState via log | RunState rebuilt from log; runtime rebuilt from CLI args |
| **LogApplier** | `BufWriter<File>` | N/A (it IS the persistence) | New file for new run |

The event loop owns the `rx`. The Dispatcher owns the `tx` (and clones it for each dispatched task).

### Dispatcher

The Dispatcher is one applier in the vec. It owns RunState plus all runtime machinery. The event loop doesn't know it's special.

```rust
struct Dispatcher {
    // Persistent state (derivable from log)
    state: RunState,

    // Runtime (transient, not persisted)
    config: Config,
    schemas: CompiledSchemas,
    step_map: HashMap<StepName, Step>,
    pool: PoolConnection,
    tx: Option<Sender<RawCompletion>>,
    in_flight: usize,
    max_concurrency: usize,
}

impl Applier for Dispatcher {
    fn apply(&mut self, entry: &StateLogEntry) {
        self.state.apply(entry);
        self.dispatch_pending();
    }

    fn process_completion(&mut self, result: &RawCompletion) -> Vec<StateLogEntry> {
        self.in_flight -= 1;
        self.build_completion_events(result)
    }
}
```

When `apply(TaskSubmitted)` fires, RunState adds the task as Pending. `dispatch_pending()` checks if there's capacity and spawns worker threads for pending tasks. When `apply(TaskCompleted)` fires, RunState transitions the task. `dispatch_pending()` runs again in case capacity freed up.

When `process_completion()` fires, the Dispatcher decrements `in_flight` and converts the raw result (stdout, exit code, timeout) into semantic state events: `TaskCompleted` with outcome, plus `TaskSubmitted` events for retries, children, or finally tasks. Those events flow back through `apply_all()`, so all appliers (including LogApplier) see them.

After each `apply()`, `dispatch_pending()` checks termination:

```rust
fn dispatch_pending(&mut self) {
    let Some(tx) = &self.tx else { return };

    while self.in_flight < self.max_concurrency {
        let Some(task_id) = self.state.next_pending() else { break };
        self.in_flight += 1;
        let tx = tx.clone();
        // ... spawn worker thread with tx clone ...
    }

    // Termination: no pending tasks, nothing in flight → drop tx
    if self.state.is_empty() && self.in_flight == 0 {
        self.tx = None;
    }
}
```

### LogApplier

Writes every event to the NDJSON state log. Zero logic.

```rust
struct LogApplier {
    writer: io::BufWriter<File>,
}

impl Applier for LogApplier {
    fn apply(&mut self, entry: &StateLogEntry) {
        barnum_state::write_entry(&mut self.writer, entry)
            .expect("failed to write state log entry");
    }
}
```

### RunState (pure state machine, not an Applier)

RunState is owned by the Dispatcher. It's a pure data structure — no I/O, no side effects.

```rust
struct RunState {
    /// All task state. Tasks not in this map are fully done.
    /// BTreeMap ordering = FIFO dispatch order (task IDs are monotonic).
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    /// Monotonic counter for assigning task IDs.
    next_task_id: u32,
}

impl RunState {
    fn apply(&mut self, entry: &StateLogEntry) {
        match entry {
            StateLogEntry::Config(_) => {}
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
                if let TaskState::WaitingForChildren { pending_children_count, .. } = &mut parent.state {
                    match NonZeroU16::new(pending_children_count.get() - 1) {
                        Some(n) => *pending_children_count = n,
                        None => {
                            // All children done — remove parent.
                            // Finally scheduling: Dispatcher checks for
                            // this in dispatch_pending() by looking for
                            // WaitingForChildren tasks with count 0.
                            // Actually, we just removed it here, so
                            // Dispatcher needs to handle finally BEFORE
                            // this removal. See Dispatcher.apply().
                            self.tasks.remove(&pid);
                        }
                    }
                }
            }
        }
    }

    /// Next pending task to dispatch (FIFO by task ID).
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
/// BEFORE
struct TaskEntry {
    step: StepName,
    parent_id: Option<LogTaskId>,
    finally_script: Option<HookScript>,  // ← config concern
    state: TaskState,
    retries_remaining: u32,              // ← dead code
}
enum TaskState {
    Pending { value: StepInputValue },
    InFlight(InFlight),                  // ← transient, not state
    WaitingForChildren {
        pending_children_count: NonZeroU16,
        finally_data: Option<(HookScript, StepInputValue)>,  // ← config concern
    },
}

/// AFTER
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

**`InFlight` removed.** Dispatch is transient — tracked by Dispatcher's `in_flight` counter. RunState sees dispatched tasks as `Pending` until `TaskCompleted` arrives. On resume, they get re-dispatched. Correct by design.

**`finally_script`, `retries_remaining` removed.** Config concerns, looked up at dispatch time.

**`finally_data` simplified to `finally_value`.** The `HookScript` comes from config. Only the value persists.

## Resume

Resume is not a special mode. It's just building the event loop and feeding it old log entries.

### Fresh run

```rust
pub fn run(runner_config: &RunnerConfig, config: Config, initial_tasks: Vec<Task>) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();

    let mut event_loop = EventLoop {
        appliers: vec![
            Box::new(Dispatcher::new(config.clone(), runner_config, tx)),
            Box::new(LogApplier::new(&runner_config.state_log_path)?),
        ],
        rx,
    };

    // Seed config
    event_loop.apply_all(StateLogEntry::Config(StateLogConfig {
        config: serde_json::to_value(&config)?,
    }));

    // Seed initial tasks
    for task in initial_tasks {
        let id = /* next_task_id from dispatcher... */;
        event_loop.apply_all(StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: id,
            step: task.step,
            value: task.value,
            parent_id: None,
            origin: TaskOrigin::Initial,
        }));
    }

    // Dispatcher.apply(TaskSubmitted) dispatched them. Now wait for completions.
    event_loop.run();
    Ok(())
}
```

### Resume

```rust
pub fn resume(old_log: &Path, runner_config: &RunnerConfig) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();

    // Read old log to extract config
    let config = extract_config_from_log(old_log)?;

    let mut event_loop = EventLoop {
        appliers: vec![
            Box::new(Dispatcher::new(config, runner_config, tx)),
            Box::new(LogApplier::new(&runner_config.state_log_path)?),
        ],
        rx,
    };

    // Replay every entry from the old log through all appliers.
    // - Dispatcher rebuilds RunState from the events.
    // - LogApplier writes them to the new log file (copying the old log).
    for entry in barnum_state::read_entries(File::open(old_log)?) {
        event_loop.apply_all(entry);
    }

    // State is reconstructed. New log has a copy of old entries.
    // Dispatcher.apply(TaskSubmitted) called dispatch_pending() for each,
    // but during replay no tx exists yet... actually, tx was passed at
    // construction, so tasks DO get dispatched during replay.
    //
    // That's wrong — we don't want to dispatch during replay because the
    // subsequent TaskCompleted entries haven't been replayed yet.
    //
    // Fix: Dispatcher starts with tx = None. After replay, set it:
    //   dispatcher.activate(tx);
    // Then dispatch_pending() becomes a no-op during replay (tx is None),
    // and only starts dispatching after activation.
    //
    // But we can't reach into the vec to call activate()...
    //
    // Simpler fix: dispatch_pending() is a no-op when tx is None.
    // The Dispatcher is constructed with tx = None for replay.
    // After replay, we do a second pass to activate:

    // Activate the dispatcher (give it the tx so it can dispatch)
    for applier in &mut event_loop.appliers {
        applier.activate();
    }

    // Now dispatch all pending tasks and enter the completion loop.
    event_loop.run();
    Ok(())
}
```

Wait — that adds an `activate()` method to the trait, which is ugly. Cleaner approach: the Dispatcher always holds the tx, but `dispatch_pending()` has a `paused` flag:

```rust
struct Dispatcher {
    // ...
    tx: Option<Sender<RawCompletion>>,
    paused: bool,  // true during replay, false during live execution
}

impl Dispatcher {
    fn dispatch_pending(&mut self) {
        if self.paused { return; }
        // ... dispatch logic ...
    }
}
```

But that's a flag that changes behavior, which the user explicitly doesn't want.

**Better approach:** Don't try to dispatch during replay at all. The event loop distinguishes seeding from running:

```rust
impl EventLoop {
    /// Feed a state event through all appliers. No side effects beyond state updates.
    fn apply_all(&mut self, entry: StateLogEntry) {
        for applier in &mut self.appliers {
            applier.apply(&entry);
        }
    }

    /// Main execution loop: wait for completions, process, dispatch.
    fn run(&mut self) {
        // First, tell all appliers we're entering the run phase.
        // This is when the Dispatcher starts dispatching pending tasks.
        for applier in &mut self.appliers {
            applier.start();
        }

        loop {
            let Ok(raw) = self.rx.recv() else { break };
            let mut events = Vec::new();
            for applier in &mut self.appliers {
                events.extend(applier.process_completion(&raw));
            }
            for event in events {
                self.apply_all(event);
            }
        }
    }
}

trait Applier {
    fn apply(&mut self, entry: &StateLogEntry);

    fn process_completion(&mut self, _result: &RawCompletion) -> Vec<StateLogEntry> {
        vec![]
    }

    /// Called once when the event loop transitions from seeding to running.
    /// The Dispatcher uses this to start dispatching pending tasks.
    fn start(&mut self) {}
}
```

The Dispatcher's `start()` gives it the signal to begin dispatching:

```rust
impl Applier for Dispatcher {
    fn apply(&mut self, entry: &StateLogEntry) {
        self.state.apply(entry);
        // During apply_all (seeding/replay), don't dispatch.
        // During run(), dispatch is triggered by start() and after each apply.
        if self.running {
            self.dispatch_pending();
        }
    }

    fn start(&mut self) {
        self.running = true;
        self.dispatch_pending();
    }

    fn process_completion(&mut self, result: &RawCompletion) -> Vec<StateLogEntry> {
        self.in_flight -= 1;
        self.build_completion_events(result)
    }
}
```

Now resume is clean:

```rust
pub fn resume(old_log: &Path, runner_config: &RunnerConfig) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    let config = extract_config_from_log(old_log)?;

    let mut event_loop = EventLoop {
        appliers: vec![
            Box::new(Dispatcher::new(config, runner_config, tx)),
            Box::new(LogApplier::new(&runner_config.state_log_path)?),
        ],
        rx,
    };

    // Replay: feed old entries through all appliers.
    // Dispatcher rebuilds RunState. LogApplier copies to new file.
    // No dispatching happens (Dispatcher.running == false).
    for entry in barnum_state::read_entries(File::open(old_log)?) {
        event_loop.apply_all(entry);
    }

    // Enter run phase: Dispatcher starts dispatching, loop processes completions.
    event_loop.run();
    Ok(())
}
```

And fresh run:

```rust
pub fn run(runner_config: &RunnerConfig, config: Config, initial_tasks: Vec<Task>) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();

    let mut event_loop = EventLoop {
        appliers: vec![
            Box::new(Dispatcher::new(config.clone(), runner_config, tx)),
            Box::new(LogApplier::new(&runner_config.state_log_path)?),
        ],
        rx,
    };

    // Seed config and initial tasks (no dispatching yet)
    event_loop.apply_all(StateLogEntry::Config(...));
    for task in initial_tasks {
        event_loop.apply_all(StateLogEntry::TaskSubmitted(...));
    }

    // Enter run phase
    event_loop.run();
    Ok(())
}
```

Both code paths are identical in structure: build event loop → seed with events → run. Resume seeds from a file. Fresh run seeds from arguments. The run phase is the same either way.

## Before/After

### TaskRunner (BEFORE)

```rust
struct TaskRunner<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    pool: PoolConnection,
    max_concurrency: usize,
    in_flight: usize,
    tx: mpsc::Sender<InFlightResult>,
    rx: mpsc::Receiver<InFlightResult>,
    next_task_id: u32,
    state_log: io::BufWriter<std::fs::File>,
}
```

Everything in one struct. State, runtime, and logging tangled together.

### EventLoop + Appliers (AFTER)

```rust
struct EventLoop {
    appliers: Vec<Box<dyn Applier>>,
    rx: Receiver<RawCompletion>,
}

struct Dispatcher {
    state: RunState,
    config: Config,
    schemas: CompiledSchemas,
    step_map: HashMap<StepName, Step>,
    pool: PoolConnection,
    tx: Option<Sender<RawCompletion>>,
    in_flight: usize,
    max_concurrency: usize,
    running: bool,
}

struct LogApplier {
    writer: io::BufWriter<File>,
}

struct RunState {
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    next_task_id: u32,
}
```

Each piece has one owner. RunState is owned by Dispatcher. Dispatcher and LogApplier are owned by the applier vec. The event loop owns the vec and the rx.

### queue_task → apply_all

```rust
// BEFORE (on TaskRunner)
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>, origin: TaskOrigin) {
    let id = self.next_task_id();
    self.log_writer.write(TaskSubmitted { task_id: id, ... });
    self.tasks.insert(id, TaskEntry { ... });
    if self.in_flight < self.max_concurrency { self.dispatch(id); }
}

// AFTER (caller constructs the event, event loop distributes it)
event_loop.apply_all(StateLogEntry::TaskSubmitted(TaskSubmitted {
    task_id: id,
    step: task.step,
    value: task.value,
    parent_id,
    origin,
}));
// Dispatcher.apply() updates RunState and dispatches if running.
// LogApplier.apply() writes to disk.
```

### task_completed → process_completion + apply_all

```rust
// BEFORE (on TaskRunner)
fn task_succeeded(&mut self, task_id: LogTaskId, spawned: Vec<Task>, value: StepInputValue) {
    self.in_flight -= 1;
    self.log_writer.write(TaskCompleted { ... });
    self.tasks.get_mut(&task_id).unwrap().state = TaskState::WaitingForChildren { ... };
}

// AFTER (Dispatcher produces events, event loop distributes them)
// In EventLoop.run():
let events = applier.process_completion(&raw);  // Dispatcher returns TaskCompleted + TaskSubmitted events
for event in events {
    event_loop.apply_all(event);  // All appliers see each event
}
```

### dispatch (internal to Dispatcher, not an event)

```rust
// BEFORE (on TaskRunner)
fn dispatch(&mut self, task_id: LogTaskId) {
    self.in_flight += 1;
    let tx = self.tx.clone();
    thread::spawn(move || { /* ... send result on tx ... */ });
}

// AFTER (on Dispatcher, same structure — just internal)
fn dispatch(&mut self, task_id: LogTaskId) {
    let entry = self.state.tasks.get(&task_id).expect("task must exist");
    let value = match &entry.state {
        TaskState::Pending { value } => value.clone(),
        _ => unreachable!("can only dispatch Pending tasks"),
    };
    self.in_flight += 1;
    let tx = self.tx.as_ref().expect("must be running").clone();
    thread::spawn(move || { /* ... send RawCompletion on tx ... */ });
}
```

## Testing

```rust
// RunState in isolation (pure state machine, no I/O)
#[test] fn apply_submitted_creates_pending_entry()
#[test] fn apply_submitted_advances_next_task_id()
#[test] fn apply_completed_success_no_children_removes_task()
#[test] fn apply_completed_success_with_children_transitions_to_waiting()
#[test] fn apply_completed_child_decrements_parent_count()
#[test] fn apply_completed_last_child_removes_parent()
#[test] fn apply_completed_failed_removes_task()

// LogApplier in isolation
#[test] fn log_applier_writes_ndjson_entries()

// Dispatcher in isolation (with mock tx/rx)
#[test] fn dispatcher_dispatches_pending_after_start()
#[test] fn dispatcher_does_not_dispatch_before_start()
#[test] fn dispatcher_drops_tx_when_done()
#[test] fn process_completion_returns_task_completed_event()
#[test] fn process_completion_returns_retry_submitted_event()

// Integration: replay
#[test] fn replay_log_reconstructs_identical_run_state()
#[test] fn replay_copies_all_entries_to_new_log()
#[test] fn replay_does_not_dispatch_during_seed()
#[test] fn replay_dispatches_pending_after_run()
```

## Migration Path

1. Define `Applier` trait (apply, process_completion, start)
2. Extract `RunState` from `TaskRunner` (pure state machine, tasks + next_task_id)
3. Build `Dispatcher` (owns RunState, pool, channels, config — implements Applier)
4. Build `LogApplier` (wraps BufWriter, implements Applier)
5. Build `EventLoop` (owns applier vec + rx, drives the loop)
6. Rewrite `run()` and `resume()` as: build EventLoop → seed → run
7. Delete old `TaskRunner`
8. Remove `InFlight` from `TaskState`, `finally_script` and `retries_remaining` from `TaskEntry`
9. Verify all tests pass
