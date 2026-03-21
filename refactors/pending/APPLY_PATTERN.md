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

## Core Idea

One method — `emit()` — updates state AND notifies all observers. Every state change goes through it. No exceptions.

```rust
fn emit(&mut self, entry: StateLogEntry) {
    self.state.apply(&entry);
    for applier in &mut self.appliers {
        applier.apply(&entry);
    }
}
```

The log is a record of what happened. `StateLogEntry` variants are facts:
- A config was loaded
- A task was submitted
- An agent returned a response
- A task completed

## StateLogEntry

```rust
enum StateLogEntry {
    Config(StateLogConfig),
    TaskSubmitted(TaskSubmitted),
    TaskCompleted(TaskCompleted),
    /// Raw agent response. RunState ignores this — it's logged for
    /// completeness and auditability. The derived TaskCompleted and
    /// TaskSubmitted entries that follow handle state changes.
    Completion(CompletionData),
}

struct CompletionData {
    task_id: LogTaskId,
    response: String,  // raw agent output
}
```

## Applier

An observer. Gets told what happened. Does not produce events, does not return anything, does not influence control flow. Pure side effect.

```rust
trait Applier {
    fn apply(&mut self, entry: &StateLogEntry);
}
```

### LogApplier

Writes every entry to the NDJSON state log. Zero logic.

```rust
struct LogApplier {
    writer: io::BufWriter<File>,
}

impl LogApplier {
    fn new(path: &Path) -> io::Result<Self> {
        let file = File::create(path)?;
        Ok(Self { writer: io::BufWriter::new(file) })
    }

    /// For resume: copy old log to new file, then append.
    fn from_existing(old_log: &Path, new_path: &Path) -> io::Result<Self> {
        std::fs::copy(old_log, new_path)?;
        let file = OpenOptions::new().append(true).open(new_path)?;
        Ok(Self { writer: io::BufWriter::new(file) })
    }
}

impl Applier for LogApplier {
    fn apply(&mut self, entry: &StateLogEntry) {
        barnum_state::write_entry(&mut self.writer, entry)
            .expect("failed to write state log entry");
    }
}
```

## Runner

This is TaskRunner refactored. It owns RunState, config, pool, channels, and the applier vec. It interprets completions. It dispatches tasks. The applier vec is notified of every state change via `emit()`.

```rust
struct Runner {
    state: RunState,
    config: Config,
    schemas: CompiledSchemas,
    step_map: HashMap<StepName, Step>,
    pool: PoolConnection,
    tx: Option<Sender<CompletionData>>,
    rx: Receiver<CompletionData>,
    in_flight: usize,
    max_concurrency: usize,
    appliers: Vec<Box<dyn Applier>>,
}
```

### emit

The single point of state mutation. Every state change goes through here.

```rust
impl Runner {
    fn emit(&mut self, entry: StateLogEntry) {
        self.state.apply(&entry);
        for applier in &mut self.appliers {
            applier.apply(&entry);
        }
    }
}
```

### Processing completions

The Runner interprets raw completions and emits facts. No cascading, no events-producing-events. Just straightforward imperative code.

```rust
impl Runner {
    fn process_completion(&mut self, completion: CompletionData) {
        self.in_flight -= 1;
        let task_id = completion.task_id;

        // Fact: the agent returned a response.
        self.emit(StateLogEntry::Completion(completion));

        // Interpret the response.
        let result = self.interpret_response(&completion);

        match result {
            Ok(parsed) => {
                // Allocate child IDs before emitting TaskCompleted
                // (TaskCompleted.spawned_task_ids references them).
                let children: Vec<_> = parsed.next_tasks.iter()
                    .map(|t| (self.state.next_id(), t.clone()))
                    .collect();
                let child_ids: Vec<_> = children.iter().map(|(id, _)| *id).collect();

                // Fact: the task completed successfully.
                self.emit(StateLogEntry::TaskCompleted(TaskCompleted {
                    task_id,
                    outcome: TaskOutcome::Success(TaskSuccess {
                        spawned_task_ids: child_ids,
                        finally_value: parsed.finally_value,
                    }),
                }));

                // Fact: child tasks were submitted.
                for (id, task) in children {
                    self.emit(StateLogEntry::TaskSubmitted(TaskSubmitted {
                        task_id: id,
                        step: task.step,
                        value: task.value,
                        parent_id: Some(task_id),
                        origin: TaskOrigin::Spawned,
                    }));
                }
            }
            Err(reason) => {
                // Maybe submit a retry.
                let retry_id = if self.should_retry(task_id) {
                    let id = self.state.next_id();
                    Some(id)
                } else {
                    None
                };

                // Fact: the task failed.
                self.emit(StateLogEntry::TaskCompleted(TaskCompleted {
                    task_id,
                    outcome: TaskOutcome::Failed(TaskFailed {
                        reason,
                        retry_task_id: retry_id,
                    }),
                }));

                // Fact: a retry was submitted.
                if let Some(retry_id) = retry_id {
                    let entry = self.state.tasks.get(&task_id).expect("task must exist");
                    self.emit(StateLogEntry::TaskSubmitted(TaskSubmitted {
                        task_id: retry_id,
                        step: entry.step.clone(),
                        value: /* original value */,
                        parent_id: entry.parent_id,
                        origin: TaskOrigin::Retry,
                    }));
                }
            }
        }

        // Handle finally for parents whose children all completed.
        for parent_id in self.state.drain_ready_for_finally() {
            let entry = self.state.tasks.get(&parent_id).expect("parent must exist");
            let TaskState::ChildrenComplete { finally_value } = &entry.state else {
                unreachable!();
            };
            let finally_script = self.step_map[&entry.step].finally.as_ref();
            if let Some(script) = finally_script {
                let id = self.state.next_id();
                self.emit(StateLogEntry::TaskSubmitted(TaskSubmitted {
                    task_id: id,
                    step: script.step.clone(),
                    value: finally_value.clone(),
                    parent_id: Some(parent_id),
                    origin: TaskOrigin::Finally,
                }));
            }
            self.state.tasks.remove(&parent_id);
        }

        self.dispatch_pending();
    }

    fn run(&mut self) {
        self.dispatch_pending();
        while let Ok(completion) = self.rx.recv() {
            self.process_completion(completion);
        }
    }
}
```

### Dispatching

```rust
impl Runner {
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

## State

### RunState

Pure state machine. No I/O, no side effects. Ignores `Completion` entries.

```rust
struct RunState {
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    next_task_id: u32,
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
    /// All children complete. Runner will submit the finally task
    /// and then remove this entry.
    ChildrenComplete { finally_value: StepInputValue },
}
```

- **`InFlight` removed.** Dispatch is transient — `in_flight` counter on Runner. RunState sees dispatched tasks as `Pending` until `TaskCompleted` arrives. On resume, they get re-dispatched.
- **`ChildrenComplete` added.** Explicit state for "children done, finally pending."
- **`finally_script`, `retries_remaining` removed.** Config concerns, looked up at dispatch time.

## Usage

### Fresh run

```rust
pub fn run(config: Config, initial_tasks: Vec<Task>, runner_config: &RunnerConfig) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    let mut state = RunState::new();
    let log_applier = LogApplier::new(&runner_config.state_log_path)?;

    let mut runner = Runner::new(state, config, runner_config, tx, rx, vec![
        Box::new(log_applier),
    ]);

    // Seed.
    runner.emit(StateLogEntry::Config(StateLogConfig { ... }));
    for task in initial_tasks {
        let id = runner.state.next_id();
        runner.emit(StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: id,
            step: task.step,
            value: task.value,
            parent_id: None,
            origin: TaskOrigin::Initial,
        }));
    }

    runner.run();
    Ok(())
}
```

### Resume

```rust
pub fn resume(old_log: &Path, runner_config: &RunnerConfig) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();
    let config = extract_config_from_log(old_log)?;

    // Rebuild RunState from the old log.
    let mut state = RunState::new();
    for entry in barnum_state::read_entries(File::open(old_log)?) {
        state.apply(&entry);
    }

    // Copy old log, then append new entries.
    let log_applier = LogApplier::from_existing(old_log, &runner_config.state_log_path)?;

    let mut runner = Runner::new(state, config, runner_config, tx, rx, vec![
        Box::new(log_applier),
    ]);

    // No seeding — state was rebuilt from the log.
    // run() dispatches pending tasks and enters the completion loop.
    runner.run();
    Ok(())
}
```

## Phasing

These phases are ordered by dependency. Each phase is a separate branch that passes CI and merges independently.

### Phase 0: Data structure cleanup

Independent refactors that can land in any order. Each is its own branch.

**0a. Extract RunState from TaskRunner.**
Move `tasks: BTreeMap<LogTaskId, TaskEntry>` and `next_task_id: u32` into a `RunState` struct. Give it its own `apply()` method. TaskRunner holds `state: RunState`. Pure structural refactor — all behavior stays in TaskRunner, RunState is just a data container with an apply method.

**0b. Remove InFlight from TaskState.**
Dispatched tasks stay as `Pending`. Track dispatch transiently with the `in_flight: usize` counter already on TaskRunner. On resume, pending-but-dispatched tasks get re-dispatched. This removes a variant from `TaskState` and simplifies the state machine.

**0c. Remove config fields from TaskEntry.**
Drop `finally_script` and `retries_remaining` from `TaskEntry`. Look them up from `step_map` at dispatch time. Config data doesn't belong in per-task state — it's the same every time and can be derived from the step name.

**0d. Add ChildrenComplete to TaskState.**
When the last child completes, transition the parent to `ChildrenComplete { finally_value }` instead of immediately removing it. Add `ready_for_finally: Vec<LogTaskId>` to RunState (or TaskRunner). The runner drains this to submit finally tasks. This makes the "children done, ready for finally" state explicit and visible.

**0e. Add Completion variant to StateLogEntry.**
Add `Completion(CompletionData)` to `StateLogEntry` in `barnum_state`. Log raw agent responses. `RunState::apply` ignores `Completion`. Update serialization. This makes the log a complete record of what happened.

### Phase 1: Introduce emit()

**Depends on: 0a (RunState exists as a field).**

Add an `emit()` method to TaskRunner that calls `self.state.apply(&entry)` AND `write_log(&mut self.state_log, &entry)`. Rewrite all state-mutating code paths (`queue_task`, `task_succeeded`, `task_failed`, etc.) to go through `emit()`.

No trait. No applier vec. Just a method on TaskRunner that ensures state and log stay in sync. This is the core behavioral change — from "manually update state AND manually write log" to "emit a fact and let emit() handle both."

Before:
```rust
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>) {
    let id = self.next_task_id();
    self.write_log(TaskSubmitted { task_id: id, ... });
    self.tasks.insert(id, TaskEntry { ... });
}
```

After:
```rust
fn queue_task(&mut self, task: Task, parent_id: Option<LogTaskId>) {
    let id = self.state.next_id();
    self.emit(StateLogEntry::TaskSubmitted(TaskSubmitted {
        task_id: id,
        step: task.step,
        value: task.value,
        parent_id,
        origin: TaskOrigin::Spawned,
    }));
}
```

### Phase 2: Extract Applier trait

**Depends on: Phase 1 (emit() exists).**

Define the `Applier` trait. Add `appliers: Vec<Box<dyn Applier>>` to TaskRunner. Move the `BufWriter` out into `LogApplier`. Change `emit()` from calling `write_log` directly to iterating over appliers.

Before (Phase 1):
```rust
fn emit(&mut self, entry: StateLogEntry) {
    self.state.apply(&entry);
    write_log(&mut self.state_log, &entry);
}
```

After (Phase 2):
```rust
fn emit(&mut self, entry: StateLogEntry) {
    self.state.apply(&entry);
    for applier in &mut self.appliers {
        applier.apply(&entry);
    }
}
```

LogApplier is the only implementation. The trait is introduced with a single impl. Future appliers (metrics, visualization, GSD state sync) can be added later without touching the runner.

### Phase 3: Seeding as construction

**Depends on: Phase 2 (applier vec exists).**

Restructure `run()` and `resume()` so RunState is built before the Runner exists. The Runner constructor takes a pre-built RunState. `run()` dispatches pending tasks and enters the completion loop — no seeding phase, no mode transitions.

For resume: build RunState from old log → copy old log to new file → construct Runner → `run()`.
For fresh run: build RunState from initial tasks (emitting seed entries to state + log) → construct Runner → `run()`.

This may require pulling seeding logic out of the Runner. Seeding writes to state and log before the Runner exists, so the caller needs direct access to RunState and LogApplier. The Runner is then constructed from the pre-seeded state.

## Before/After Summary

| Before (`TaskRunner`) | After (`Runner`) |
|---|---|
| `queue_task` manually writes log AND updates state | `emit()` does both atomically |
| `task_succeeded`/`task_failed` manually writes log AND updates state | `process_completion` calls `emit()` for each fact |
| `BufWriter` as a field, inline log writes | `LogApplier` in the applier vec |
| `InFlight` variant in `TaskState` | `in_flight: usize` on Runner only |
| `finally_script` on `TaskEntry` | Looked up from `step_map` at dispatch time |
| Raw completions not logged | `Completion` variant — log is a complete record |
| Resume requires separate code path | Same shape: build state → construct → run |

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

// Runner: emit guarantees
#[test] fn emit_updates_state_and_notifies_appliers()
#[test] fn process_completion_emits_completion_then_task_completed()
#[test] fn process_completion_emits_child_submitted_entries()
#[test] fn process_completion_emits_retry_on_failure()
#[test] fn process_completion_handles_finally_for_ready_parents()

// Runner: dispatch
#[test] fn dispatches_up_to_max_concurrency()
#[test] fn drops_tx_when_empty_and_no_in_flight()

// Integration: replay
#[test] fn replay_reconstructs_identical_run_state()
#[test] fn replay_copies_entries_to_new_log()
#[test] fn resumed_run_dispatches_incomplete_tasks()
```
