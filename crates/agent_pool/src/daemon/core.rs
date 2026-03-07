//! Core: Pure State Machine
//!
//! This module contains the core state machine logic with zero I/O dependencies.
//! All types are pure data, and the `step` function is a pure transformation:
//!
//! ```text
//! step(state, event) -> (state, effects)
//! ```
//!
//! Key design principles:
//! - **No I/O**: No filesystem, no sockets, no time
//! - **No config**: Core is purely reactive
//! - **IDs only**: I/O layer maps IDs to actual content/channels
//! - **Deterministic**: Same input always produces same output
//!
//! # Anonymous Worker Model
//!
//! Workers are **one-shot**: they register, receive one task, complete it, and are
//! removed. To process another task, a worker must re-register with a new UUID.
//!
//! This simplifies the state machine:
//! - No "idle" state after task completion
//! - No deregistration events (workers just disappear)
//! - Either tasks are waiting for workers, or workers are waiting for tasks, never both

use std::collections::{HashMap, VecDeque};

// =============================================================================
// ID Types
// =============================================================================

/// Unique identifier for a worker.
///
/// Workers are anonymous - they have no "name", just an ID assigned on registration.
/// I/O layer maps `WorkerId` to the actual UUID and file paths.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct WorkerId(pub(super) u32);

impl From<u32> for WorkerId {
    fn from(id: u32) -> Self {
        WorkerId(id)
    }
}

/// Unique identifier for a submission (external task).
///
/// I/O layer maps `SubmissionId` to the actual UUID and response channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct SubmissionId(pub(super) u32);

impl From<u32> for SubmissionId {
    fn from(id: u32) -> Self {
        SubmissionId(id)
    }
}

/// Task identifier - either an external submission or a heartbeat.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum TaskId {
    External(SubmissionId),
    Heartbeat,
}

// =============================================================================
// Pool State
// =============================================================================

/// Either tasks are waiting for workers, or workers are waiting for tasks, or neither.
/// Never both - any arrival triggers immediate matching.
///
/// Invariant: `VecDeque` is always non-empty in Tasks/Workers variants.
/// When the last item is removed, transition to None.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) enum Waiting {
    #[default]
    None,
    Tasks(VecDeque<SubmissionId>),
    Workers(VecDeque<WorkerId>),
}

/// The complete state of the worker pool.
///
/// Workers are one-shot: they register, get assigned a task, complete it, and are
/// removed. The `busy_workers` map tracks workers that have been assigned tasks.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PoolState {
    /// What's currently waiting (tasks or workers or nothing)
    pub(super) waiting: Waiting,
    /// Workers that have been assigned tasks (`worker_id` -> `task_id`)
    pub(super) busy_workers: HashMap<WorkerId, TaskId>,
}

impl PoolState {
    /// Create a new empty pool.
    #[must_use]
    pub(super) fn new() -> Self {
        Self::default()
    }

    /// Number of pending tasks.
    #[must_use]
    pub(super) fn pending_count(&self) -> usize {
        match &self.waiting {
            Waiting::Tasks(tasks) => tasks.len(),
            _ => 0,
        }
    }

    /// Number of idle workers (waiting for tasks).
    #[must_use]
    pub(super) fn idle_count(&self) -> usize {
        match &self.waiting {
            Waiting::Workers(workers) => workers.len(),
            _ => 0,
        }
    }

    /// Number of busy workers.
    #[must_use]
    pub(super) fn busy_count(&self) -> usize {
        self.busy_workers.len()
    }

    /// Total number of workers (idle + busy).
    #[must_use]
    pub(super) fn worker_count(&self) -> usize {
        self.idle_count() + self.busy_count()
    }
}

// =============================================================================
// Events (Inputs)
// =============================================================================

/// Events that can affect pool state.
///
/// Named in past tense - these are things that HAPPENED.
/// I/O layer detects these events and sends them to core.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Event {
    /// A task was submitted by a client.
    TaskSubmitted { submission_id: SubmissionId },

    /// A worker registered (wrote ready.json).
    WorkerReady { worker_id: WorkerId },

    /// A worker completed its task (wrote response.json).
    WorkerResponded { worker_id: WorkerId },

    /// A worker timed out while processing a task.
    WorkerTimedOut { worker_id: WorkerId },

    /// Request to assign a heartbeat task to an idle worker.
    /// Only succeeds if the worker is still idle.
    AssignHeartbeatIfIdle { worker_id: WorkerId },
}

// =============================================================================
// Effects (Outputs)
// =============================================================================

/// Effects that I/O layer should execute.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Effect {
    /// A task was assigned to a worker.
    /// I/O layer: write task.json, start timeout timer.
    TaskAssigned {
        worker_id: WorkerId,
        task_id: TaskId,
    },

    /// A worker is idle and waiting for work.
    /// I/O layer: start idle timeout timer for heartbeat.
    WorkerWaiting { worker_id: WorkerId },

    /// A task was completed successfully.
    /// I/O layer: read response, send to submitter, clean up files.
    TaskCompleted {
        worker_id: WorkerId,
        task_id: TaskId,
    },

    /// A task failed (worker timed out).
    /// I/O layer: send error response to submitter.
    TaskFailed { submission_id: SubmissionId },

    /// A worker was removed (timed out or after completing task).
    /// I/O layer: clean up worker files, cancel timers.
    WorkerRemoved { worker_id: WorkerId },
}

// =============================================================================
// Step Function
// =============================================================================

/// Pure state transition function.
///
/// Given the current state and an event, returns the new state and effects.
#[must_use]
pub(super) fn step(state: PoolState, event: Event) -> (PoolState, Vec<Effect>) {
    match event {
        Event::TaskSubmitted { submission_id } => handle_task_submitted(state, submission_id),
        Event::WorkerReady { worker_id } => handle_worker_ready(state, worker_id),
        Event::WorkerResponded { worker_id } => handle_worker_responded(state, worker_id),
        Event::WorkerTimedOut { worker_id } => handle_worker_timed_out(state, worker_id),
        Event::AssignHeartbeatIfIdle { worker_id } => {
            handle_assign_heartbeat_if_idle(state, worker_id)
        }
    }
}

// =============================================================================
// Event Handlers
// =============================================================================

#[expect(clippy::expect_used)] // Invariant: Workers variant is always non-empty
fn handle_task_submitted(
    mut state: PoolState,
    submission_id: SubmissionId,
) -> (PoolState, Vec<Effect>) {
    match &mut state.waiting {
        Waiting::Workers(worker_ids) => {
            // Workers waiting - assign to first one
            let worker_id = worker_ids
                .pop_front()
                .expect("Workers variant with empty queue");
            if worker_ids.is_empty() {
                state.waiting = Waiting::None;
            }
            state
                .busy_workers
                .insert(worker_id, TaskId::External(submission_id));
            (
                state,
                vec![Effect::TaskAssigned {
                    worker_id,
                    task_id: TaskId::External(submission_id),
                }],
            )
        }
        Waiting::Tasks(submission_ids) => {
            // Already have pending tasks - add to queue
            submission_ids.push_back(submission_id);
            (state, vec![])
        }
        Waiting::None => {
            // Nothing waiting - start task queue
            state.waiting = Waiting::Tasks(VecDeque::from([submission_id]));
            (state, vec![])
        }
    }
}

#[expect(clippy::expect_used)] // Invariant: Tasks variant is always non-empty
fn handle_worker_ready(mut state: PoolState, worker_id: WorkerId) -> (PoolState, Vec<Effect>) {
    // PANIC: IO layer guarantees WorkerReady is sent exactly once per worker.
    assert!(
        !state.busy_workers.contains_key(&worker_id),
        "WorkerReady for already-busy worker - IO layer bug"
    );

    match &mut state.waiting {
        Waiting::Tasks(submission_ids) => {
            // Tasks waiting - assign first one to this worker
            let submission_id = submission_ids
                .pop_front()
                .expect("Tasks variant with empty queue");
            if submission_ids.is_empty() {
                state.waiting = Waiting::None;
            }
            state
                .busy_workers
                .insert(worker_id, TaskId::External(submission_id));
            (
                state,
                vec![Effect::TaskAssigned {
                    worker_id,
                    task_id: TaskId::External(submission_id),
                }],
            )
        }
        Waiting::Workers(worker_ids) => {
            // Other workers already waiting - add to queue
            assert!(
                !worker_ids.contains(&worker_id),
                "Same worker appearing twice in waiting queue - IO layer bug"
            );
            worker_ids.push_back(worker_id);
            (state, vec![Effect::WorkerWaiting { worker_id }])
        }
        Waiting::None => {
            // Nothing waiting - start worker queue
            state.waiting = Waiting::Workers(VecDeque::from([worker_id]));
            (state, vec![Effect::WorkerWaiting { worker_id }])
        }
    }
}

fn handle_worker_responded(mut state: PoolState, worker_id: WorkerId) -> (PoolState, Vec<Effect>) {
    // DEFENSIVE: Worker might not be in busy_workers if:
    // - Timeout fired first and already removed the worker
    let Some(task_id) = state.busy_workers.remove(&worker_id) else {
        return (state, vec![]);
    };

    // Worker completed task - remove it (one-shot model)
    (
        state,
        vec![
            Effect::TaskCompleted { worker_id, task_id },
            Effect::WorkerRemoved { worker_id },
        ],
    )
}

fn handle_worker_timed_out(mut state: PoolState, worker_id: WorkerId) -> (PoolState, Vec<Effect>) {
    // DEFENSIVE: Worker might have already responded
    let Some(task_id) = state.busy_workers.remove(&worker_id) else {
        return (state, vec![]);
    };

    let mut effects = vec![Effect::WorkerRemoved { worker_id }];
    if let TaskId::External(submission_id) = task_id {
        effects.push(Effect::TaskFailed { submission_id });
    }
    (state, effects)
}

fn handle_assign_heartbeat_if_idle(
    mut state: PoolState,
    worker_id: WorkerId,
) -> (PoolState, Vec<Effect>) {
    // DEFENSIVE: Timer event - worker state may have changed since timer was scheduled.
    let Waiting::Workers(worker_ids) = &mut state.waiting else {
        return (state, vec![]); // No idle workers at all
    };
    let Some(pos) = worker_ids.iter().position(|&id| id == worker_id) else {
        return (state, vec![]); // This specific worker not idle (busy or gone)
    };

    worker_ids.remove(pos);
    if worker_ids.is_empty() {
        state.waiting = Waiting::None;
    }
    state.busy_workers.insert(worker_id, TaskId::Heartbeat);
    (
        state,
        vec![Effect::TaskAssigned {
            worker_id,
            task_id: TaskId::Heartbeat,
        }],
    )
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn sub(id: u32) -> SubmissionId {
        SubmissionId(id)
    }

    fn worker(id: u32) -> WorkerId {
        WorkerId(id)
    }

    fn ext(id: u32) -> TaskId {
        TaskId::External(SubmissionId(id))
    }

    // -------------------------------------------------------------------------
    // Task Submission Tests
    // -------------------------------------------------------------------------

    #[test]
    fn task_submitted_queues_when_no_workers() {
        let state = PoolState::new();
        let (state, effects) = step(
            state,
            Event::TaskSubmitted {
                submission_id: sub(1),
            },
        );

        assert_eq!(state.pending_count(), 1);
        assert!(effects.is_empty());
    }

    #[test]
    fn task_submitted_dispatches_to_idle_worker() {
        let mut state = PoolState::new();
        state.waiting = Waiting::Workers(VecDeque::from([worker(1)]));

        let (state, effects) = step(
            state,
            Event::TaskSubmitted {
                submission_id: sub(42),
            },
        );

        assert_eq!(state.pending_count(), 0);
        assert_eq!(state.busy_count(), 1);
        assert!(state.busy_workers.contains_key(&worker(1)));

        assert_eq!(effects.len(), 1);
        assert!(matches!(
            &effects[0],
            Effect::TaskAssigned { worker_id, task_id }
                if *worker_id == worker(1) && *task_id == ext(42)
        ));
    }

    #[test]
    fn task_submitted_queues_when_all_workers_busy() {
        let mut state = PoolState::new();
        state.busy_workers.insert(worker(1), ext(99));

        let (state, effects) = step(
            state,
            Event::TaskSubmitted {
                submission_id: sub(42),
            },
        );

        assert_eq!(state.pending_count(), 1);
        assert!(effects.is_empty());
    }

    // -------------------------------------------------------------------------
    // Task Withdrawal Tests
    // -------------------------------------------------------------------------
    // Worker Registration Tests
    // -------------------------------------------------------------------------

    #[test]
    fn worker_ready_with_no_tasks_waits() {
        let state = PoolState::new();
        let (state, effects) = step(
            state,
            Event::WorkerReady {
                worker_id: worker(1),
            },
        );

        assert_eq!(state.idle_count(), 1);
        assert_eq!(state.busy_count(), 0);

        assert_eq!(effects.len(), 1);
        assert!(matches!(
            &effects[0],
            Effect::WorkerWaiting { worker_id } if *worker_id == worker(1)
        ));
    }

    #[test]
    fn worker_ready_dispatches_pending_task() {
        let mut state = PoolState::new();
        state.waiting = Waiting::Tasks(VecDeque::from([sub(42)]));

        let (state, effects) = step(
            state,
            Event::WorkerReady {
                worker_id: worker(1),
            },
        );

        assert_eq!(state.pending_count(), 0);
        assert_eq!(state.busy_count(), 1);

        assert_eq!(effects.len(), 1);
        assert!(matches!(
            &effects[0],
            Effect::TaskAssigned { worker_id, task_id }
                if *worker_id == worker(1) && *task_id == ext(42)
        ));
    }

    // -------------------------------------------------------------------------
    // Worker Response Tests
    // -------------------------------------------------------------------------

    #[test]
    fn worker_responded_completes_and_removes() {
        let mut state = PoolState::new();
        state.busy_workers.insert(worker(1), ext(42));

        let (state, effects) = step(
            state,
            Event::WorkerResponded {
                worker_id: worker(1),
            },
        );

        assert_eq!(state.busy_count(), 0);
        assert_eq!(state.worker_count(), 0);

        assert_eq!(effects.len(), 2);
        assert!(matches!(
            &effects[0],
            Effect::TaskCompleted { worker_id, task_id }
                if *worker_id == worker(1) && *task_id == ext(42)
        ));
        assert!(matches!(
            &effects[1],
            Effect::WorkerRemoved { worker_id } if *worker_id == worker(1)
        ));
    }

    #[test]
    fn worker_responded_noop_for_unknown() {
        let state = PoolState::new();
        let (state, effects) = step(
            state,
            Event::WorkerResponded {
                worker_id: worker(999),
            },
        );

        assert!(effects.is_empty());
        assert_eq!(state.worker_count(), 0);
    }

    // -------------------------------------------------------------------------
    // Timeout Tests
    // -------------------------------------------------------------------------

    #[test]
    fn worker_timeout_removes_and_fails_task() {
        let mut state = PoolState::new();
        state.busy_workers.insert(worker(1), ext(42));

        let (state, effects) = step(
            state,
            Event::WorkerTimedOut {
                worker_id: worker(1),
            },
        );

        assert_eq!(state.busy_count(), 0);

        assert_eq!(effects.len(), 2);
        assert!(matches!(
            &effects[0],
            Effect::WorkerRemoved { worker_id } if *worker_id == worker(1)
        ));
        assert!(matches!(
            &effects[1],
            Effect::TaskFailed { submission_id } if *submission_id == sub(42)
        ));
    }

    #[test]
    fn worker_timeout_noop_if_already_responded() {
        let state = PoolState::new();
        let (state, effects) = step(
            state,
            Event::WorkerTimedOut {
                worker_id: worker(1),
            },
        );

        assert!(effects.is_empty());
        assert_eq!(state.worker_count(), 0);
    }

    // -------------------------------------------------------------------------
    // Heartbeat Tests
    // -------------------------------------------------------------------------

    #[test]
    fn heartbeat_assigns_to_idle_worker() {
        let mut state = PoolState::new();
        state.waiting = Waiting::Workers(VecDeque::from([worker(1)]));

        let (state, effects) = step(
            state,
            Event::AssignHeartbeatIfIdle {
                worker_id: worker(1),
            },
        );

        assert_eq!(state.idle_count(), 0);
        assert_eq!(state.busy_count(), 1);
        assert_eq!(state.busy_workers.get(&worker(1)), Some(&TaskId::Heartbeat));

        assert_eq!(effects.len(), 1);
        assert!(matches!(
            &effects[0],
            Effect::TaskAssigned { worker_id, task_id }
                if *worker_id == worker(1) && *task_id == TaskId::Heartbeat
        ));
    }

    #[test]
    fn heartbeat_noop_if_worker_busy() {
        let mut state = PoolState::new();
        state.busy_workers.insert(worker(1), ext(42));

        let (state, effects) = step(
            state,
            Event::AssignHeartbeatIfIdle {
                worker_id: worker(1),
            },
        );

        // Worker still busy with original task
        assert_eq!(state.busy_workers.get(&worker(1)), Some(&ext(42)));
        assert!(effects.is_empty());
    }

    #[test]
    fn heartbeat_noop_if_worker_gone() {
        let state = PoolState::new();
        let (state, effects) = step(
            state,
            Event::AssignHeartbeatIfIdle {
                worker_id: worker(1),
            },
        );

        assert!(effects.is_empty());
        assert_eq!(state.worker_count(), 0);
    }

    // -------------------------------------------------------------------------
    // Determinism Tests
    // -------------------------------------------------------------------------

    #[test]
    fn task_dispatches_to_first_idle_worker() {
        let mut state = PoolState::new();
        state.waiting = Waiting::Workers(VecDeque::from([worker(3), worker(1), worker(2)]));

        let (state, effects) = step(
            state,
            Event::TaskSubmitted {
                submission_id: sub(1),
            },
        );

        // Should dispatch to worker(3) - first in queue (FIFO)
        assert_eq!(state.idle_count(), 2);
        assert!(state.busy_workers.contains_key(&worker(3)));

        assert!(matches!(
            &effects[0],
            Effect::TaskAssigned { worker_id, .. } if *worker_id == worker(3)
        ));
    }
}
