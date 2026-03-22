# Engine Unit Testing: Separate State Transitions from Dispatch

**Status:** Not started

**Depends on:** APPLY_PATTERN (done)

## Motivation

The Engine in `crates/barnum_config/src/runner/mod.rs` mixes two concerns:

1. **State transitions** — given an event, mutate the task tree and decide what work is pending
2. **Dispatch** — spawn threads, submit to pools, run shell commands

These are tangled inside Engine: `apply_entry` mutates `self.state` and pushes to `self.pending_dispatches`, then `flush_dispatches` reads from `self.pending_dispatches` and spawns threads. Both live on the same struct, so testing `apply_entry` requires constructing a full Engine with a pool connection, channel, schemas, etc.

The state transition logic is the hardest code to get right (parent chain walks, child count arithmetic, finally detection, retry replacement) and the easiest to unit test — if it were accessible without I/O dependencies.

## Current State

`Engine` (`mod.rs:354`) owns everything:

```rust
struct Engine<'a> {
    config: &'a Config,
    schemas: &'a CompiledSchemas,
    step_map: HashMap<&'a StepName, &'a Step>,
    state: RunState,               // pure state
    pool: PoolConnection,          // I/O
    tx: mpsc::Sender<WorkerResult>, // I/O
    max_concurrency: usize,        // dispatch policy
    in_flight: usize,              // dispatch tracking
    pending_dispatches: VecDeque<PendingDispatch>, // straddles both
    dropped_count: u32,            // result tracking
}
```

`RunState` (`mod.rs:107`) is already pure — no I/O, just `BTreeMap<LogTaskId, TaskEntry>` and `next_task_id`. Its methods (`apply_submitted`, `apply_completed`, `apply_finally_run`, `walk_up_for_finally`) are pure state mutations. But they're not independently testable because `apply_entry` on Engine orchestrates the calls and manages `pending_dispatches`.

The flow today:

```
apply_entry(StateLogEntry)
  ├── advance_id_to(...)        // RunState
  ├── apply_submitted(...)       // RunState
  ├── apply_completed(...)       // RunState — returns parent_id
  ├── walk_up_for_finally(...)   // RunState — returns finally_id
  ├── pending_dispatches.push_back(...)  // Engine field
  └── in_flight.saturating_sub(1)        // Engine field
```

Then `flush_dispatches` reads `pending_dispatches` and spawns threads.

## Proposed Change

Move `pending_dispatches` onto `RunState`. Add a method on `RunState` that takes a `StateLogEntry`, mutates the task tree, and accumulates pending dispatches — but never dispatches anything. Engine calls this method, then drains the pending dispatches into actual thread spawns.

### RunState after

```rust
struct RunState {
    tasks: BTreeMap<LogTaskId, TaskEntry>,
    next_task_id: u32,
    pending_dispatches: VecDeque<PendingDispatch>,
}
```

New method:

```rust
impl RunState {
    /// Apply a single entry to state. Queues pending dispatches but does not
    /// execute them. Caller is responsible for draining `pending_dispatches`.
    fn apply_entry(&mut self, entry: &StateLogEntry, config: &Config) {
        match entry {
            StateLogEntry::Config(_) => {}
            StateLogEntry::TaskSubmitted(s) => {
                self.advance_id_to(s.task_id.0 + 1);
                self.apply_submitted(s);
                self.pending_dispatches
                    .push_back(PendingDispatch::Task { task_id: s.task_id });
            }
            StateLogEntry::TaskCompleted(c) => {
                // Remove stale pending dispatch for this task (replay case)
                self.pending_dispatches.retain(
                    |d| !matches!(d, PendingDispatch::Task { task_id } if *task_id == c.task_id),
                );

                // Advance IDs from embedded children/retries
                // ... (same logic as current apply_entry)

                let parent_id = self.apply_completed(c);

                // Queue children/retry
                // ... (same logic)

                // Walk up for finally
                if let Some(pid) = parent_id
                    && let Some(finally_id) = self.walk_up_for_finally(pid, config)
                {
                    self.pending_dispatches
                        .push_back(PendingDispatch::Finally { parent_id: finally_id });
                }
            }
            StateLogEntry::FinallyRun(f) => {
                // ... (same logic)
            }
        }
    }

    /// Apply a batch of entries.
    fn apply_entries(&mut self, entries: &[StateLogEntry], config: &Config) {
        for entry in entries {
            self.apply_entry(entry, config);
        }
    }
}
```

Engine becomes thinner — it owns RunState and handles I/O:

```rust
impl Engine<'_> {
    fn apply_and_dispatch(&mut self, entries: &[StateLogEntry]) {
        self.state.apply_entries(entries, self.config);
        self.flush_dispatches();
    }

    fn flush_dispatches(&mut self) {
        while self.in_flight < self.max_concurrency {
            let Some(dispatch) = self.state.pending_dispatches.pop_front() else {
                break;
            };
            self.in_flight += 1;
            // ... spawn thread based on dispatch variant
        }
    }
}
```

### What moves where

| Field | Before | After |
|-------|--------|-------|
| `tasks` | RunState | RunState (unchanged) |
| `next_task_id` | RunState | RunState (unchanged) |
| `pending_dispatches` | Engine | RunState |
| `in_flight` | Engine | Engine (unchanged) |
| `pool`, `tx` | Engine | Engine (unchanged) |
| `config`, `schemas`, `step_map` | Engine | Engine (unchanged, passed as args to RunState methods) |
| `dropped_count` | Engine | Engine (unchanged) |

### `in_flight` tracking during replay

Currently `apply_entry` decrements `in_flight` on `TaskCompleted` and `FinallyRun`. This is Engine-level concern (tracking actual workers), not state-level. During replay `in_flight` is 0 so the `saturating_sub` is harmless, but it shouldn't be in RunState.

The clean split: RunState doesn't know about `in_flight`. Engine adjusts `in_flight` based on what entries it processes. When Engine receives a `WorkerResult` (live execution), it decrements `in_flight` before calling `state.apply_entry`. During replay, `in_flight` stays 0.

## Unit Tests

With this separation, RunState is testable with no I/O at all:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn make_state() -> RunState {
        RunState::new()
    }

    fn seed(id: u32, step: &str) -> StateLogEntry {
        StateLogEntry::TaskSubmitted(TaskSubmitted {
            task_id: LogTaskId(id),
            step: StepName::new(step),
            value: StepInputValue(serde_json::json!({})),
            origin: TaskOrigin::Seed,
        })
    }

    #[test]
    fn seed_task_queues_dispatch() {
        let mut state = make_state();
        let config = /* minimal config with one step */;

        state.apply_entry(&seed(0, "Analyze"), &config);

        // Task is in the map
        assert!(state.tasks.contains_key(&LogTaskId(0)));
        // Dispatch was queued
        assert_eq!(state.pending_dispatches.len(), 1);
        assert!(matches!(
            state.pending_dispatches[0],
            PendingDispatch::Task { task_id: LogTaskId(0) }
        ));
    }

    #[test]
    fn completed_with_children_queues_child_dispatches() {
        let mut state = make_state();
        let config = /* ... */;

        state.apply_entry(&seed(0, "Analyze"), &config);
        state.pending_dispatches.clear(); // drain seed dispatch

        let completed = StateLogEntry::TaskCompleted(TaskCompleted {
            task_id: LogTaskId(0),
            outcome: barnum_state::TaskOutcome::Success(barnum_state::TaskSuccess {
                finally_value: StepInputValue(serde_json::json!({})),
                children: vec![
                    TaskSubmitted { task_id: LogTaskId(1), step: StepName::new("Review"), /* ... */ },
                    TaskSubmitted { task_id: LogTaskId(2), step: StepName::new("Review"), /* ... */ },
                ],
            }),
        });
        state.apply_entry(&completed, &config);

        // Two child dispatches queued
        assert_eq!(state.pending_dispatches.len(), 2);
        // Parent is in WaitingForChildren
        assert!(matches!(
            state.tasks[&LogTaskId(0)].state,
            TaskState::WaitingForChildren(_)
        ));
    }

    #[test]
    fn finally_detected_when_all_children_complete() {
        // Setup: parent with finally hook, one child
        // Complete the child
        // Assert: PendingDispatch::Finally { parent_id } is queued
    }

    #[test]
    fn replay_removes_stale_dispatches() {
        // Apply seed, then immediately apply completed
        // The seed's PendingDispatch::Task should be removed
    }

    #[test]
    fn retry_replaces_failed_task() {
        // Seed → Completed(Failed with retry) → retry is in map, original removed
    }
}
```

These tests exercise the exact state machine logic that currently has zero direct unit test coverage — it's only tested indirectly through the integration tests that spawn real agents.

## Relationship to Existing Refactors

- **SYNC_TESTING_HARNESS.md** — about troupe's daemon (Transport trait, mock agents). Orthogonal. That's about testing the pool dispatch layer; this is about testing barnum's task orchestration logic.
- **TEST_HARNESS_IMPROVEMENTS.md** — about troupe's test infrastructure (CLI vs raw file modes). Also orthogonal.

Neither is obsolete — they target different layers. This refactor targets barnum's Engine, not troupe.

## Scope

Small refactor — move `pending_dispatches` from Engine to RunState, move `apply_entry` logic from Engine to RunState (passing `config` as arg), adjust Engine to drain dispatches after calling RunState. Then add unit tests.

No behavioral change. Same state transitions, same dispatch logic, just split across the struct boundary differently.
