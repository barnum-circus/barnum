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
//! # Epoch-Based Timeout Validation
//!
//! Timeouts are tricky in distributed systems. A timer might fire after the
//! agent has already done work, making the timeout stale. We handle this with
//! epochs:
//!
//! - Each agent has an `Epoch` (`agent_id` + sequence number)
//! - Sequence increments on every state transition (idle→busy, busy→idle)
//! - Timeout events carry the epoch they were created with
//! - If current epoch doesn't match, the timeout is stale and ignored

use std::collections::{BTreeMap, VecDeque};

// =============================================================================
// ID Types
// =============================================================================

/// External task ID - a real submission from a client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct ExternalTaskId(pub(super) u32);

/// Heartbeat ID - a synthetic task to validate agent liveness.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct HeartbeatId(pub(super) u32);

/// Task identifier - either an external submission or a heartbeat.
///
/// Core treats both variants uniformly for scheduling. The I/O layer
/// uses the variant to determine response handling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum TaskId {
    External(ExternalTaskId),
    Heartbeat(HeartbeatId),
}

/// Unique identifier for a registered agent.
///
/// Agents are anonymous - they have no "name", just an ID assigned on registration.
/// I/O layer maps `AgentId` to the actual communication channel.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(super) struct AgentId(pub(super) u32);

impl From<u32> for AgentId {
    fn from(id: u32) -> Self {
        AgentId(id)
    }
}

/// Agent epoch - identifies a specific point in an agent's lifecycle.
///
/// Used to validate timeout events. Contains:
/// - `agent_id`: Which agent this epoch belongs to
/// - `sequence`: Increments on every state transition
///
/// When a timeout fires, we check if the agent's current epoch matches.
/// If not, the agent has done work since the timer started, so we ignore it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct Epoch {
    /// Which agent this epoch belongs to.
    pub(super) agent_id: AgentId,
    /// Increments on every state transition (idle→busy, busy→idle).
    pub(super) sequence: u32,
}

// =============================================================================
// Agent State
// =============================================================================

/// What an agent is currently doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentStatus {
    /// Ready to receive work.
    Idle,
    /// Currently processing a task (real task or heartbeat - core doesn't distinguish).
    Busy {
        /// The task being processed (heartbeat tasks have IDs too - I/O layer tracks which).
        task_id: TaskId,
    },
}

/// Complete state for a single agent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct AgentState {
    /// What the agent is currently doing
    pub(super) status: AgentStatus,
    /// Current epoch - increments on every state transition
    pub(super) epoch: Epoch,
}

impl AgentState {
    /// Create a new agent in idle state with initial epoch.
    const fn new(agent_id: AgentId) -> Self {
        Self {
            status: AgentStatus::Idle,
            epoch: Epoch {
                agent_id,
                sequence: 0,
            },
        }
    }

    /// Check if the agent is idle (ready for work).
    #[must_use]
    pub(super) const fn is_idle(&self) -> bool {
        matches!(self.status, AgentStatus::Idle)
    }

    /// Try to transition from idle to busy. Returns `Some(new_epoch)` on success,
    /// `None` if agent is already busy.
    ///
    /// Works for both real tasks and heartbeat tasks - core doesn't distinguish.
    #[allow(clippy::missing_const_for_fn)]
    fn try_become_busy(&mut self, task_id: TaskId) -> Option<Epoch> {
        if !self.is_idle() {
            return None;
        }
        self.epoch.sequence += 1;
        self.status = AgentStatus::Busy { task_id };
        Some(self.epoch)
    }

    /// Try to transition from busy to idle. Returns `Some((new_epoch, task_id))` on success,
    /// `None` if agent is already idle.
    #[allow(clippy::missing_const_for_fn)]
    fn try_become_idle(&mut self) -> Option<(Epoch, TaskId)> {
        let AgentStatus::Busy { task_id } = self.status else {
            return None;
        };
        self.epoch.sequence += 1;
        self.status = AgentStatus::Idle;
        Some((self.epoch, task_id))
    }
}

// =============================================================================
// Pool State
// =============================================================================

/// The complete state of the agent pool.
///
/// This is the "world" that the state machine operates on. It contains:
/// - A queue of pending tasks (just IDs - content is in I/O layer)
/// - A map of registered agents and their states
///
/// `BTreeMap` is used for agents to ensure deterministic iteration order,
/// which helps with debugging and snapshot testing.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct PoolState {
    /// Tasks waiting to be assigned to agents (FIFO queue)
    pending_tasks: VecDeque<TaskId>,
    /// Registered agents and their current state
    agents: BTreeMap<AgentId, AgentState>,
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
        self.pending_tasks.len()
    }

    /// Number of registered agents.
    #[must_use]
    pub(super) fn agent_count(&self) -> usize {
        self.agents.len()
    }

    /// Number of busy agents (test helper).
    #[cfg(test)]
    fn busy_count(&self) -> usize {
        self.agents.values().filter(|a| !a.is_idle()).count()
    }

    /// Number of idle agents (test helper).
    #[cfg(test)]
    fn idle_count(&self) -> usize {
        self.agents.values().filter(|a| a.is_idle()).count()
    }

    /// Check if an agent is registered (test helper).
    #[cfg(test)]
    fn has_agent(&self, agent_id: AgentId) -> bool {
        self.agents.contains_key(&agent_id)
    }

    /// Get agent state (test helper).
    #[cfg(test)]
    fn get_agent(&self, agent_id: AgentId) -> Option<&AgentState> {
        self.agents.get(&agent_id)
    }

    /// Check if a task is pending (test helper).
    #[cfg(test)]
    fn has_pending_task(&self, task_id: TaskId) -> bool {
        self.pending_tasks.contains(&task_id)
    }
}

// =============================================================================
// Events (Inputs)
// =============================================================================

/// Events that can affect pool state.
///
/// Named in past tense - these are things that HAPPENED.
/// I/O layer detects these events and sends them to core.
///
/// Note: Events carry minimal data. Content, channels, and other
/// "real world" concerns live in I/O layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Event {
    /// A task was submitted.
    /// I/O layer has stored the content and response channel.
    TaskSubmitted {
        /// The submitted task's ID.
        task_id: TaskId,
    },

    /// A task was withdrawn (submitter disconnected before completion).
    /// Remove from pending queue if not yet dispatched.
    #[allow(dead_code)] // Not yet used but part of the API
    TaskWithdrawn {
        /// The withdrawn task's ID.
        task_id: TaskId,
    },

    /// An agent registered (directory appeared or socket connected).
    /// I/O layer assigns the `AgentId` and optionally provides a heartbeat task ID.
    AgentRegistered {
        /// The registered agent's ID.
        agent_id: AgentId,
        /// Heartbeat task to assign immediately, or None to try pending queue.
        heartbeat_task_id: Option<TaskId>,
    },

    /// An agent deregistered (directory removed or socket closed).
    AgentDeregistered {
        /// The deregistered agent's ID.
        agent_id: AgentId,
    },

    /// An agent completed its work (wrote `response.json` or sent response).
    /// I/O layer has stored the response content.
    AgentResponded {
        /// The responding agent's ID.
        agent_id: AgentId,
    },

    /// A task timeout fired for an agent (agent was busy or waiting for heartbeat).
    /// Core checks if the epoch matches; if not, ignores the stale timeout.
    AgentTimedOut {
        /// The epoch when the timer was started.
        epoch: Epoch,
    },

    /// Request to assign a task directly to a specific agent (bypassing queue).
    /// Used for heartbeats, and in the future could support priority/affinity.
    /// Only succeeds if the agent's current epoch matches - otherwise the agent
    /// has done work since this request was created and we ignore it.
    AssignTaskToAgentIfEpochMatches {
        /// The epoch when this assignment was requested.
        epoch: Epoch,
        /// The task to assign (pre-allocated by I/O layer).
        task_id: TaskId,
    },

    /// Shutdown signal from I/O layer.
    /// Event loop should exit immediately.
    Shutdown,
}

// =============================================================================
// Effects (Outputs)
// =============================================================================

/// Effects that I/O layer should execute.
///
/// Named in past tense like Events - these describe what happened in the
/// state machine that I/O layer needs to act on.
///
/// Note: Effects carry minimal data. I/O layer looks up content, channels,
/// and other details using the IDs provided.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Effect {
    /// A task was assigned to an agent.
    /// I/O layer: write `task.json`, start timeout timer using epoch.
    TaskAssigned {
        /// The assigned task.
        task_id: TaskId,
        /// The agent's epoch when assigned (for timeout validation).
        epoch: Epoch,
    },

    /// An agent became idle (after registration or task completion).
    /// I/O layer: start idle timeout timer using epoch.
    AgentIdled {
        /// The agent's current epoch (for timeout validation).
        epoch: Epoch,
    },

    /// A task was completed successfully.
    /// I/O layer: read response from agent, send to submitter.
    TaskCompleted {
        /// The agent that completed the task.
        agent_id: AgentId,
        /// The completed task.
        task_id: TaskId,
    },

    /// A task failed (agent timed out while processing).
    /// I/O layer: send error response to submitter.
    TaskFailed {
        /// The failed task.
        task_id: TaskId,
    },

    /// An agent was removed (timed out or deregistered).
    /// I/O layer: clean up the agent's channel.
    AgentRemoved {
        /// The removed agent.
        agent_id: AgentId,
    },
}

// =============================================================================
// Step Function
// =============================================================================

/// Pure state transition function.
///
/// This is the heart of the core. Given the current state and an event,
/// it returns the new state and a list of effects to execute.
///
/// Properties:
/// - **Pure**: No I/O, no side effects
/// - **Deterministic**: Same inputs always produce same outputs
/// - **Total**: Handles any event in any state gracefully
#[must_use]
pub(super) fn step(state: PoolState, event: Event) -> (PoolState, Vec<Effect>) {
    match event {
        Event::TaskSubmitted { task_id } => handle_task_submitted(state, task_id),
        Event::TaskWithdrawn { task_id } => handle_task_withdrawn(state, task_id),
        Event::AgentRegistered {
            agent_id,
            heartbeat_task_id,
        } => handle_agent_registered(state, agent_id, heartbeat_task_id),
        Event::AgentDeregistered { agent_id } => handle_agent_deregistered(state, agent_id),
        Event::AgentResponded { agent_id } => handle_agent_responded(state, agent_id),
        Event::AgentTimedOut { epoch } => handle_agent_timed_out(state, epoch),
        Event::AssignTaskToAgentIfEpochMatches { epoch, task_id } => {
            handle_assign_task_to_agent_if_epoch_matches(state, epoch, task_id)
        }
        // Shutdown is handled by the event loop before calling step.
        // If it reaches here, that's a bug.
        Event::Shutdown => {
            unreachable!("Event::Shutdown should be handled by event loop, not step()")
        }
    }
}

// =============================================================================
// Event Handlers
// =============================================================================

fn handle_task_submitted(mut state: PoolState, task_id: TaskId) -> (PoolState, Vec<Effect>) {
    if let Some(effect) = try_assign_task_to_idle_agent(&mut state, task_id) {
        (state, vec![effect])
    } else {
        state.pending_tasks.push_back(task_id);
        (state, vec![])
    }
}

fn handle_task_withdrawn(mut state: PoolState, task_id: TaskId) -> (PoolState, Vec<Effect>) {
    // Remove from pending queue if not yet dispatched
    if let Some(pos) = state.pending_tasks.iter().position(|&id| id == task_id) {
        state.pending_tasks.remove(pos);
    }
    // If already dispatched, we can't recall it. The response will be
    // discarded when TaskCompleted is processed (I/O layer won't find a responder).
    (state, vec![])
}

#[allow(clippy::expect_used)] // Invariant: we just inserted the agent
fn handle_agent_registered(
    mut state: PoolState,
    agent_id: AgentId,
    heartbeat_task_id: Option<TaskId>,
) -> (PoolState, Vec<Effect>) {
    let agent = AgentState::new(agent_id);
    let initial_epoch = agent.epoch;

    let old = state.agents.insert(agent_id, agent);
    assert!(
        old.is_none(),
        "duplicate AgentRegistered event - I/O layer bug"
    );

    if let Some(task_id) = heartbeat_task_id {
        // Heartbeat provided - assign it directly
        let agent = state.agents.get_mut(&agent_id).expect("just inserted");
        let epoch = agent
            .try_become_busy(task_id)
            .expect("new agent should be idle");
        (state, vec![Effect::TaskAssigned { task_id, epoch }])
    } else if let Some(effect) = try_assign_pending_to_agent(&mut state, agent_id) {
        // Assigned a pending task
        (state, vec![effect])
    } else {
        // No work available
        (
            state,
            vec![Effect::AgentIdled {
                epoch: initial_epoch,
            }],
        )
    }
}

fn handle_agent_deregistered(mut state: PoolState, agent_id: AgentId) -> (PoolState, Vec<Effect>) {
    let Some(agent) = state.agents.remove(&agent_id) else {
        // Agent already removed - benign race between timeout-based removal and
        // filesystem events (agent timed out, got kicked, deleted its directory,
        // FSWatcher saw deletion and sent this event).
        return (state, vec![]);
    };

    match agent.status {
        AgentStatus::Idle => (state, vec![]),
        AgentStatus::Busy { task_id } => (state, vec![Effect::TaskFailed { task_id }]),
    }
}

fn handle_agent_responded(mut state: PoolState, agent_id: AgentId) -> (PoolState, Vec<Effect>) {
    let Some(agent) = state.agents.get_mut(&agent_id) else {
        // Agent was removed (e.g., kicked due to timeout) between when it wrote
        // response.json and when we processed this event. The response is stale.
        return (state, vec![]);
    };

    let Some((new_epoch, task_id)) = agent.try_become_idle() else {
        // Agent is already idle - this is a duplicate FS event that arrived after
        // we processed the first response. Safe to ignore.
        return (state, vec![]);
    };

    let mut effects = vec![Effect::TaskCompleted { agent_id, task_id }];

    if let Some(effect) = try_assign_pending_to_agent(&mut state, agent_id) {
        effects.push(effect);
    } else {
        effects.push(Effect::AgentIdled { epoch: new_epoch });
    }

    (state, effects)
}

fn handle_agent_timed_out(mut state: PoolState, epoch: Epoch) -> (PoolState, Vec<Effect>) {
    let agent_id = epoch.agent_id;

    let Some(agent) = state.agents.remove(&agent_id) else {
        return (state, vec![]);
    };

    if agent.epoch != epoch {
        // Stale timeout - agent did work since timer started
        state.agents.insert(agent_id, agent);
        return (state, vec![]);
    }

    match agent.status {
        AgentStatus::Busy { task_id } => (
            state,
            vec![
                Effect::TaskFailed { task_id },
                Effect::AgentRemoved { agent_id },
            ],
        ),
        // Invariant: epoch matches but agent idle is impossible.
        // Timeout timers are created when agent becomes Busy, and epoch increments
        // on every state transition. If epoch matches, agent must still be Busy.
        AgentStatus::Idle => {
            unreachable!("AgentTimedOut with matching epoch but idle - daemon bug");
        }
    }
}

#[allow(clippy::expect_used)] // Invariant: epoch match implies idle state
fn handle_assign_task_to_agent_if_epoch_matches(
    mut state: PoolState,
    epoch: Epoch,
    task_id: TaskId,
) -> (PoolState, Vec<Effect>) {
    let agent_id = epoch.agent_id;

    let Some(agent) = state.agents.get_mut(&agent_id) else {
        return (state, vec![]);
    };

    if agent.epoch != epoch {
        return (state, vec![]);
    }

    // Epoch matches, so agent must be idle (becoming busy increments epoch)
    let new_epoch = agent
        .try_become_busy(task_id)
        .expect("epoch matched but agent not idle - epoch logic bug");

    (
        state,
        vec![Effect::TaskAssigned {
            task_id,
            epoch: new_epoch,
        }],
    )
}

// =============================================================================
// Dispatch Logic
// =============================================================================

/// Try to assign a specific task to any idle agent.
///
/// Used when a task is submitted and we need to find an agent for it.
#[allow(clippy::expect_used)]
fn try_assign_task_to_idle_agent(state: &mut PoolState, task_id: TaskId) -> Option<Effect> {
    let idle_count = state.agents.values().filter(|a| a.is_idle()).count();
    let total_count = state.agents.len();
    tracing::debug!(
        ?task_id,
        idle_count,
        total_count,
        "try_assign_task_to_idle_agent"
    );

    let agent_id = state
        .agents
        .iter()
        .find(|(_, agent)| agent.is_idle())
        .map(|(&id, _)| id)?;

    let agent = state
        .agents
        .get_mut(&agent_id)
        .expect("just found this agent");

    let epoch = agent
        .try_become_busy(task_id)
        .expect("just verified agent is idle");

    Some(Effect::TaskAssigned { task_id, epoch })
}

/// Try to assign a pending task to a specific agent.
///
/// Used when we know which agent should receive work (e.g., after registration
/// or task completion).
#[allow(clippy::expect_used)]
fn try_assign_pending_to_agent(state: &mut PoolState, agent_id: AgentId) -> Option<Effect> {
    let task_id = state.pending_tasks.pop_front()?;

    let agent = state
        .agents
        .get_mut(&agent_id)
        .expect("caller guarantees agent exists");

    let epoch = agent
        .try_become_busy(task_id)
        .expect("caller guarantees agent is idle");

    Some(Effect::TaskAssigned { task_id, epoch })
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    /// Helper to create external task IDs in tests.
    fn ext(id: u32) -> TaskId {
        TaskId::External(ExternalTaskId(id))
    }

    // -------------------------------------------------------------------------
    // Task Submission Tests
    // -------------------------------------------------------------------------

    #[test]
    fn task_submitted_queues_when_no_agents() {
        let state = PoolState::new();
        let (state, effects) = step(state, Event::TaskSubmitted { task_id: ext(1) });

        assert_eq!(state.pending_count(), 1);
        assert!(state.has_pending_task(ext(1)));
        assert!(effects.is_empty(), "No agents, so no dispatch");
    }

    #[test]
    fn task_submitted_dispatches_to_idle_agent() {
        let mut state = PoolState::new();
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));

        let (state, effects) = step(state, Event::TaskSubmitted { task_id: ext(42) });

        assert_eq!(state.pending_count(), 0, "Task should be dispatched");
        assert_eq!(state.busy_count(), 1);

        let agent = state.get_agent(AgentId(1)).unwrap();
        assert_eq!(agent.status, AgentStatus::Busy { task_id: ext(42) });
        assert_eq!(
            agent.epoch.sequence, 1,
            "Epoch should increment on dispatch"
        );

        assert_eq!(effects.len(), 1);
        assert!(matches!(
            &effects[0],
            Effect::TaskAssigned { task_id, epoch }
                if *task_id == ext(42) && epoch.agent_id == AgentId(1) && epoch.sequence == 1
        ));
    }

    #[test]
    fn task_submitted_queues_when_all_agents_busy() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        agent.try_become_busy(ext(99)).unwrap();
        state.agents.insert(AgentId(1), agent);

        let (state, effects) = step(state, Event::TaskSubmitted { task_id: ext(42) });

        assert_eq!(state.pending_count(), 1);
        assert!(state.has_pending_task(ext(42)));
        assert!(effects.is_empty());
    }

    #[test]
    fn task_submitted_dispatches_one_task_to_one_agent() {
        let mut state = PoolState::new();
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));
        state.agents.insert(AgentId(2), AgentState::new(AgentId(2)));

        // Submit a task - should dispatch to exactly one agent
        let (state, effects) = step(state, Event::TaskSubmitted { task_id: ext(1) });

        assert_eq!(state.busy_count(), 1);
        assert_eq!(state.idle_count(), 1);
        assert_eq!(state.pending_count(), 0);
        assert_eq!(effects.len(), 1);
        assert!(matches!(&effects[0], Effect::TaskAssigned { task_id, .. } if *task_id == ext(1)));
    }

    // -------------------------------------------------------------------------
    // Task Withdrawal Tests
    // -------------------------------------------------------------------------

    #[test]
    fn task_withdrawn_removes_from_pending() {
        let mut state = PoolState::new();
        state.pending_tasks.push_back(ext(1));
        state.pending_tasks.push_back(ext(2));
        state.pending_tasks.push_back(ext(3));

        let (state, effects) = step(state, Event::TaskWithdrawn { task_id: ext(2) });

        assert_eq!(state.pending_count(), 2);
        assert!(!state.has_pending_task(ext(2)));
        assert!(state.has_pending_task(ext(1)));
        assert!(state.has_pending_task(ext(3)));
        assert!(effects.is_empty());
    }

    #[test]
    fn task_withdrawn_noop_for_unknown_task() {
        let state = PoolState::new();
        let (state, effects) = step(state, Event::TaskWithdrawn { task_id: ext(999) });

        assert_eq!(state.pending_count(), 0);
        assert!(effects.is_empty());
    }

    #[test]
    fn task_withdrawn_noop_for_dispatched_task() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        agent.try_become_busy(ext(42)).unwrap();
        state.agents.insert(AgentId(1), agent);

        // Try to withdraw a task that's already dispatched
        let (state, effects) = step(state, Event::TaskWithdrawn { task_id: ext(42) });

        // Task is still being processed - can't recall it
        assert_eq!(state.busy_count(), 1);
        assert!(effects.is_empty());
    }

    // -------------------------------------------------------------------------
    // Agent Registration Tests
    // -------------------------------------------------------------------------

    #[test]
    fn agent_registered_adds_to_pool() {
        let state = PoolState::new();
        let (state, effects) = step(
            state,
            Event::AgentRegistered {
                agent_id: AgentId(1),
                heartbeat_task_id: None,
            },
        );

        assert!(state.has_agent(AgentId(1)));
        assert_eq!(state.idle_count(), 1);

        assert_eq!(effects.len(), 1);
        assert!(matches!(
            &effects[0],
            Effect::AgentIdled { epoch }
                if epoch.agent_id == AgentId(1) && epoch.sequence == 0
        ));
    }

    #[test]
    fn agent_registered_dispatches_pending_task() {
        let mut state = PoolState::new();
        state.pending_tasks.push_back(ext(42));

        let (state, effects) = step(
            state,
            Event::AgentRegistered {
                agent_id: AgentId(1),
                heartbeat_task_id: None,
            },
        );

        assert_eq!(state.pending_count(), 0);
        assert_eq!(state.busy_count(), 1);

        // Only TaskAssigned - no AgentIdled since agent is immediately busy
        assert_eq!(effects.len(), 1);
        assert!(matches!(
            &effects[0],
            Effect::TaskAssigned { task_id, .. } if *task_id == ext(42)
        ));
    }

    // -------------------------------------------------------------------------
    // Agent Deregistration Tests
    // -------------------------------------------------------------------------

    #[test]
    fn agent_deregistered_removes_idle_agent() {
        let mut state = PoolState::new();
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));

        let (state, effects) = step(
            state,
            Event::AgentDeregistered {
                agent_id: AgentId(1),
            },
        );

        assert!(!state.has_agent(AgentId(1)));
        assert!(effects.is_empty(), "Idle agent deregister has no effects");
    }

    #[test]
    fn agent_deregistered_fails_in_flight_task() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        agent.try_become_busy(ext(42)).unwrap();
        state.agents.insert(AgentId(1), agent);

        let (state, effects) = step(
            state,
            Event::AgentDeregistered {
                agent_id: AgentId(1),
            },
        );

        assert!(!state.has_agent(AgentId(1)));
        assert_eq!(effects.len(), 1);
        assert!(matches!(
            &effects[0],
            Effect::TaskFailed { task_id } if *task_id == ext(42)
        ));
    }

    #[test]
    fn agent_deregistered_noop_for_unknown() {
        let state = PoolState::new();
        let (state, effects) = step(
            state,
            Event::AgentDeregistered {
                agent_id: AgentId(999),
            },
        );

        assert_eq!(state.agent_count(), 0);
        assert!(effects.is_empty());
    }

    // -------------------------------------------------------------------------
    // Agent Response Tests
    // -------------------------------------------------------------------------

    #[test]
    fn agent_responded_completes_task() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        agent.try_become_busy(ext(42)).unwrap();
        state.agents.insert(AgentId(1), agent);

        let (state, effects) = step(
            state,
            Event::AgentResponded {
                agent_id: AgentId(1),
            },
        );

        let agent = state.get_agent(AgentId(1)).unwrap();
        assert!(agent.is_idle());
        assert_eq!(agent.epoch.sequence, 2, "Epoch should be 2 (busy + idle)");

        assert_eq!(effects.len(), 2);
        assert!(matches!(
            &effects[0],
            Effect::TaskCompleted { agent_id: AgentId(1), task_id } if *task_id == ext(42)
        ));
        assert!(matches!(
            &effects[1],
            Effect::AgentIdled { epoch } if epoch.sequence == 2
        ));
    }

    #[test]
    fn agent_responded_dispatches_next_task() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        agent.try_become_busy(ext(1)).unwrap();
        state.agents.insert(AgentId(1), agent);
        state.pending_tasks.push_back(ext(2));

        let (state, effects) = step(
            state,
            Event::AgentResponded {
                agent_id: AgentId(1),
            },
        );

        let agent = state.get_agent(AgentId(1)).unwrap();
        assert_eq!(agent.status, AgentStatus::Busy { task_id: ext(2) });
        assert_eq!(
            agent.epoch.sequence, 3,
            "Epoch: 1 (busy) + 2 (idle) + 3 (busy)"
        );

        // No AgentIdled when immediately dispatching next task
        assert_eq!(effects.len(), 2);
        assert!(matches!(&effects[0], Effect::TaskCompleted { .. }));
        assert!(matches!(
            &effects[1],
            Effect::TaskAssigned { task_id, .. } if *task_id == ext(2)
        ));
    }

    #[test]
    fn agent_responded_noop_for_unknown_agent() {
        let state = PoolState::new();
        let (state, effects) = step(
            state,
            Event::AgentResponded {
                agent_id: AgentId(999),
            },
        );

        assert!(effects.is_empty());
        assert_eq!(state.agent_count(), 0);
    }

    #[test]
    fn agent_responded_ignores_idle_agent() {
        let mut state = PoolState::new();
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));

        // Idle agent responding is ignored (duplicate FS event race condition)
        let (new_state, effects) = step(
            state,
            Event::AgentResponded {
                agent_id: AgentId(1),
            },
        );

        assert!(effects.is_empty());
        assert_eq!(new_state.agent_count(), 1);
    }

    // -------------------------------------------------------------------------
    // Timeout Tests
    // -------------------------------------------------------------------------

    #[test]
    #[should_panic(expected = "daemon bug")]
    fn timeout_idle_agent_with_matching_epoch_panics() {
        // This scenario shouldn't happen - daemon should send heartbeat on AgentIdled,
        // making the agent busy. If we get here, it's a daemon bug.
        let mut state = PoolState::new();
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));
        let epoch = state.get_agent(AgentId(1)).unwrap().epoch;

        let _ = step(state, Event::AgentTimedOut { epoch });
    }

    #[test]
    fn timeout_deregisters_busy_agent_and_fails_task() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        let epoch = agent.try_become_busy(ext(42)).unwrap();
        state.agents.insert(AgentId(1), agent);

        let (state, effects) = step(state, Event::AgentTimedOut { epoch });

        assert!(!state.has_agent(AgentId(1)));
        assert_eq!(effects.len(), 2);
        // TaskFailed should come first (logical ordering)
        assert!(matches!(
            &effects[0],
            Effect::TaskFailed { task_id } if *task_id == ext(42)
        ));
        assert!(matches!(
            &effects[1],
            Effect::AgentRemoved {
                agent_id: AgentId(1)
            }
        ));
    }

    #[test]
    fn stale_timeout_ignored_epoch_mismatch() {
        let mut state = PoolState::new();
        let mut agent = AgentState::new(AgentId(1));
        let old_epoch = agent.epoch;
        agent.try_become_busy(ext(42)).unwrap(); // Epoch is now 1
        state.agents.insert(AgentId(1), agent);

        // Fire timeout with old epoch (sequence 0)
        let (state, effects) = step(state, Event::AgentTimedOut { epoch: old_epoch });

        // Agent should still exist
        assert!(state.has_agent(AgentId(1)));
        assert!(effects.is_empty());
    }

    #[test]
    fn stale_timeout_ignored_agent_gone() {
        let state = PoolState::new();
        let epoch = Epoch {
            agent_id: AgentId(999),
            sequence: 0,
        };

        let (state, effects) = step(state, Event::AgentTimedOut { epoch });

        assert!(effects.is_empty());
        assert_eq!(state.agent_count(), 0);
    }

    // -------------------------------------------------------------------------
    // Epoch Sequence Tests
    // -------------------------------------------------------------------------

    #[test]
    fn epoch_increments_correctly_through_lifecycle() {
        let state = PoolState::new();

        // Register agent - epoch 0
        let (mut state, effects) = step(
            state,
            Event::AgentRegistered {
                agent_id: AgentId(1),
                heartbeat_task_id: None,
            },
        );
        let epoch0 = match &effects[0] {
            Effect::AgentIdled { epoch } => *epoch,
            _ => panic!("Expected AgentIdled"),
        };
        assert_eq!(epoch0.sequence, 0);

        // Submit task - agent becomes busy, epoch 1
        state.pending_tasks.push_back(ext(1));
        let (state, effects) = step(state, Event::TaskSubmitted { task_id: ext(2) });
        // First dispatch is task 1
        let epoch1 = match &effects[0] {
            Effect::TaskAssigned { epoch, .. } => *epoch,
            _ => panic!("Expected TaskAssigned"),
        };
        assert_eq!(epoch1.sequence, 1);

        // Agent responds with pending task - goes directly to busy, epoch 3
        // (epoch 2 is the brief idle state, epoch 3 is busy with task 2)
        let (_state, effects) = step(
            state,
            Event::AgentResponded {
                agent_id: AgentId(1),
            },
        );
        // No AgentIdled when immediately dispatching next task
        assert_eq!(effects.len(), 2);
        assert!(matches!(&effects[0], Effect::TaskCompleted { .. }));
        let epoch3 = match &effects[1] {
            Effect::TaskAssigned { epoch, .. } => *epoch,
            _ => panic!("Expected TaskAssigned"),
        };
        assert_eq!(
            epoch3.sequence, 3,
            "Epoch: 1 (first busy) → 2 (idle) → 3 (second busy)"
        );
    }

    // -------------------------------------------------------------------------
    // Determinism Tests
    // -------------------------------------------------------------------------

    #[test]
    fn agent_selection_is_deterministic() {
        // BTreeMap ensures agents are iterated in ID order, so lowest ID is selected
        let mut state = PoolState::new();
        state.agents.insert(AgentId(3), AgentState::new(AgentId(3)));
        state.agents.insert(AgentId(1), AgentState::new(AgentId(1)));
        state.agents.insert(AgentId(2), AgentState::new(AgentId(2)));

        let (_state, effects) = step(state, Event::TaskSubmitted { task_id: ext(1) });

        // Should dispatch to AgentId(1) - lowest ID
        assert_eq!(effects.len(), 1);
        assert!(matches!(
            &effects[0],
            Effect::TaskAssigned { task_id, epoch } if *task_id == ext(1) && epoch.agent_id == AgentId(1)
        ));
    }
}
