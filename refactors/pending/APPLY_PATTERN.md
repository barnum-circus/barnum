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

Same pattern as Troupe's daemon (`crates/troupe/src/daemon`). `step()` updates state and produces entries. A for loop dispatches each entry to appliers. State and log stay in sync because every state change produces the entry that the log applier writes.

## StateLogEntry

```rust
enum StateLogEntry {
    Config(StateLogConfig),
    TaskSubmitted(TaskSubmitted),
    TaskCompleted(TaskCompleted),
}
```

Each variant is a fact. Task removal is derived inside `RunState::apply()` when all children complete.

## Applier

```rust
trait Applier {
    fn apply(&mut self, entry: &StateLogEntry);
}
```

Appliers receive entries after state has been updated. They do not return values or influence control flow.

### LogApplier

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

## RunState

Pure dependency tracker. No I/O, no config awareness, no knowledge of "finally." Tracks tasks and parent-child relationships. When a task completes with no children, it's removed and its parent's child count is decremented. If the count reaches zero, the parent is removed and its own parent's count is decremented, continuing up the tree.

Parents whose count reaches zero are accumulated in `removed_parents`. `step()` drains this after each `apply()` call to check whether they need finally tasks (a business-logic decision that belongs in `step()`, not in the state machine).

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

        let Some(pid) = parent_id else { return };
        let Some(parent) = self.tasks.get_mut(&pid) else { return };
        let TaskState::WaitingForChildren {
            pending_children_count, finally_value
        } = &mut parent.state else { return };

        match NonZeroU16::new(pending_children_count.get() - 1) {
            Some(n) => *pending_children_count = n,
            None => {
                // All children done. Capture parent info before removal.
                let step = parent.step.clone();
                let fv = finally_value.clone();
                self.removed_parents.push(RemovedParent {
                    task_id: pid,
                    step,
                    finally_value: fv,
                });
                // Remove parent and recurse up the tree.
                self.remove_and_notify_parent(pid);
            }
        }
    }

    fn drain_removed_parents(&mut self) -> Vec<RemovedParent> {
        std::mem::take(&mut self.removed_parents)
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
    Pending { value: StepInputValue },
    WaitingForChildren {
        pending_children_count: NonZeroU16,
        finally_value: StepInputValue,
    },
}
```

Two variants. `InFlight` is gone (dispatch tracked by `in_flight: usize` on Runner). Config fields (`finally_script`, `retries_remaining`) are gone (looked up from `step_map` at dispatch time).

## step()

Interprets a completion, updates `RunState`, handles finally for any removed parents, and returns entries for appliers.

```rust
fn step(
    state: &mut RunState,
    config: &Config,
    step_map: &HashMap<StepName, Step>,
    completion: CompletionData,
) -> Vec<StateLogEntry> {
    let mut entries = Vec::new();
    let task_id = completion.task_id;
    let result = interpret_response(config, &completion);

    match result {
        Ok(parsed) => {
            let children: Vec<_> = parsed.next_tasks.iter()
                .map(|t| (state.next_id(), t.clone()))
                .collect();
            let child_ids: Vec<_> = children.iter().map(|(id, _)| *id).collect();

            let completed = StateLogEntry::TaskCompleted(TaskCompleted {
                task_id,
                outcome: TaskOutcome::Success(TaskSuccess {
                    spawned_task_ids: child_ids,
                    finally_value: parsed.finally_value,
                }),
            });
            state.apply(&completed);
            entries.push(completed);

            for (id, task) in children {
                let submitted = StateLogEntry::TaskSubmitted(TaskSubmitted {
                    task_id: id,
                    step: task.step,
                    value: task.value,
                    parent_id: Some(task_id),
                    origin: TaskOrigin::Spawned,
                });
                state.apply(&submitted);
                entries.push(submitted);
            }
        }
        Err(reason) => {
            // Extract retry info before applying (apply removes the task).
            let retry_submitted = if should_retry(config, task_id) {
                let retry_id = state.next_id();
                let task = state.tasks.get(&task_id)
                    .expect("task must exist");
                Some((retry_id, TaskSubmitted {
                    task_id: retry_id,
                    step: task.step.clone(),
                    value: task.pending_value(),
                    parent_id: task.parent_id,
                    origin: TaskOrigin::Retry,
                }))
            } else {
                None
            };

            let failed = StateLogEntry::TaskCompleted(TaskCompleted {
                task_id,
                outcome: TaskOutcome::Failed(TaskFailed {
                    reason,
                    retry_task_id: retry_submitted.as_ref().map(|(id, _)| *id),
                }),
            });
            state.apply(&failed);
            entries.push(failed);

            if let Some((_, submitted)) = retry_submitted {
                let entry = StateLogEntry::TaskSubmitted(submitted);
                state.apply(&entry);
                entries.push(entry);
            }
        }
    }

    // Handle finally for parents whose children all completed.
    // This is business logic (config lookup), not state tracking.
    for parent in state.drain_removed_parents() {
        if let Some(script) = step_map.get(&parent.step)
            .and_then(|s| s.finally.as_ref())
        {
            let id = state.next_id();
            let submitted = StateLogEntry::TaskSubmitted(TaskSubmitted {
                task_id: id,
                step: script.step.clone(),
                value: parent.finally_value,
                parent_id: None, // parent is already removed
                origin: TaskOrigin::Finally,
            });
            state.apply(&submitted);
            entries.push(submitted);
        }
    }

    entries
}
```

## Runner

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

impl Runner {
    fn run(&mut self) {
        self.dispatch_pending();
        while let Ok(completion) = self.rx.recv() {
            self.in_flight -= 1;
            let entries = step(
                &mut self.state, &self.config,
                &self.step_map, completion,
            );
            for entry in &entries {
                for applier in &mut self.appliers {
                    applier.apply(entry);
                }
            }
            self.dispatch_pending();
        }
    }

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

### Termination

Worker threads hold `Sender<CompletionData>` clones. They drop them after sending their completion. Runner drops its sender when state is empty and `in_flight` is 0. With all senders dropped, `rx.recv()` returns `Err` and the loop exits.

## Usage

```rust
pub fn run(
    config: Config,
    initial_tasks: Vec<Task>,
    runner_config: &RunnerConfig,
) -> io::Result<()> {
    let (tx, rx) = mpsc::channel();

    let mut state = RunState::new();
    let seed_entries = build_seed_entries(&mut state, &config, &initial_tasks);

    let file = File::create(&runner_config.state_log_path)?;
    let mut log_applier = LogApplier { writer: io::BufWriter::new(file) };
    for entry in &seed_entries {
        state.apply(entry);
        log_applier.apply(entry);
    }

    let mut runner = Runner {
        state,
        config,
        schemas: CompiledSchemas::compile(&config)?,
        step_map: build_step_map(&config),
        pool: PoolConnection::new(runner_config)?,
        tx: Some(tx),
        rx,
        in_flight: 0,
        max_concurrency: runner_config.max_concurrency,
        appliers: vec![Box::new(log_applier)],
    };

    runner.run();
    Ok(())
}

fn build_seed_entries(
    state: &mut RunState,
    config: &Config,
    tasks: &[Task],
) -> Vec<StateLogEntry> {
    let mut entries = vec![StateLogEntry::Config(StateLogConfig { ... })];
    for task in tasks {
        let id = state.next_id();
        entries.push(StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: id,
            step: task.step.clone(),
            value: task.value.clone(),
            parent_id: None,
            origin: TaskOrigin::Initial,
        }));
    }
    entries
}
```

## Phasing

Each phase is a separate branch that passes CI and merges independently.

### Phase 0: Data structure cleanup

Independent refactors that can land in any order.

**0a.** Extract `RunState` from `TaskRunner`. Move `tasks: BTreeMap<LogTaskId, TaskEntry>` and `next_task_id: u32` into a `RunState` struct with its own `apply()` method. `TaskRunner` holds `state: RunState`. Pure structural move.

**0b.** Remove `InFlight` from `TaskState`. Dispatched tasks stay `Pending`. Track dispatch count with `in_flight: usize` on TaskRunner.

**0c.** Remove config fields from `TaskEntry`. Drop `finally_script` and `retries_remaining`. Look them up from `step_map` at dispatch time.

**0d.** Make parent removal derived. When the last child completes, remove the parent inside `apply()` and capture the removed parent's info in `removed_parents: Vec<RemovedParent>`. The runner drains this to check config for finally scripts. This replaces the current inline finally handling with a clean interface: RunState reports what happened, the runner decides what to do about it.

### Phase 1: step() and applier for loop

**Depends on: 0a.**

Extract a `step()` function that interprets completions, calls `state.apply()` for each resulting entry, and returns the entries. Introduce the `Applier` trait and `LogApplier`. Rewrite the main loop to call `step()` and dispatch entries to appliers via the inner for loop.

### Phase 2: Seeding as construction

**Depends on: Phase 1.**

Restructure `run()` so state is built and seeded before Runner exists. `build_seed_entries` produces entries. The caller applies them to state and writes them to the log. Runner takes the pre-seeded state.

## Before/After

| Before (`TaskRunner`) | After (`Runner`) |
|---|---|
| `queue_task` manually writes log AND updates state | `step()` updates state and returns entries; for loop writes via appliers |
| `task_succeeded`/`task_failed` manually writes log AND updates state | Same: all state changes go through `step()` |
| `BufWriter` as a field, inline log writes | `LogApplier` behind the `Applier` trait |
| `InFlight` variant in `TaskState` | `in_flight: usize` on Runner |
| `finally_script` on `TaskEntry` | Looked up from `step_map` at dispatch time |
| `ChildrenComplete`, `has_finally`, `ready_for_finally` | `removed_parents` vec; RunState has no config awareness |

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
#[test] fn step_drains_removed_parents_and_submits_finally()
#[test] fn step_no_finally_script_skips_removed_parent()
#[test] fn step_failure_produces_completed()
#[test] fn step_failure_with_retry_produces_completed_then_submitted()

// Runner
#[test] fn run_dispatches_up_to_max_concurrency()
#[test] fn run_drops_tx_when_empty_and_no_in_flight()
#[test] fn seed_entries_applied_before_run()
#[test] fn replay_log_reconstructs_identical_run_state()
```
