# Three-Layer Daemon Refactor

## Design Principles

### Serial Event Processing

Events are processed **serially, one at a time**. Layer 2 receives events from a channel and processes them in order:

```rust
while let Ok(event) = events_rx.recv() {
    let (new_state, effects) = step(state, event);
    state = new_state;
    // execute effects...
}
```

The mental model: a **linear vector of events** that we process sequentially. No concurrent state mutation, no race conditions within the state machine.

### Byzantine Resilience

The outside world is adversarial. Events can arrive in any order:
- Agent responds before being registered? Handle it.
- Response arrives for unknown task? Ignore it.
- Duplicate registration? Idempotent.
- Event for deregistered agent? No-op.

The `step()` function must be **resilient to any event sequence**. We never assume:
- Events arrive in "logical" order
- FS events are reliable or ordered
- Agents behave correctly

Each event is handled based solely on **current state**, not assumptions about what "should have" happened before.

### Determinism

Given the same `(state, event)` pair, `step()` always returns the same `(state, effects)`. No randomness, no time, no I/O. This makes the state machine:
- **Testable**: Unit tests are deterministic
- **Debuggable**: Replay event sequence to reproduce bugs
- **Auditable**: Log events, reconstruct state at any point

---

## Target Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│                     Layer 3: I/O                                 │
│                                                                  │
│  Responsibilities:                                              │
│  - Listen on socket, accept connections, read/write             │
│  - Watch filesystem, handle fs events                           │
│  - Parse JSON messages                                          │
│  - Execute effects (send responses via socket or file)          │
│                                                                  │
│  Communicates with Layer 2 via channels                         │
└────────────────────────────────┬────────────────────────────────┘
                                 │ Event enum
                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                     Layer 2: Event Loop                          │
│                                                                  │
│  Responsibilities:                                              │
│  - Hold PoolState                                               │
│  - Receive Events from Layer 3                                  │
│  - Call step(state, event) → (state, effects)                   │
│  - Send Effects back to Layer 3 for execution                   │
│                                                                  │
│  Pure orchestration - no I/O, no business logic                 │
└────────────────────────────────┬────────────────────────────────┘
                                 │ step(state, event)
                                 ▼
┌─────────────────────────────────────────────────────────────────┐
│                     Layer 1: Pure State Machine                  │
│                                                                  │
│  fn step(state: PoolState, event: Event) -> (PoolState, Vec<Effect>)
│                                                                  │
│  Responsibilities:                                              │
│  - All business logic (dispatch, health checks, etc.)           │
│  - State transitions                                            │
│  - Decide what effects to emit                                  │
│                                                                  │
│  Pure function - no I/O, no channels, trivially testable        │
└─────────────────────────────────────────────────────────────────┘
```

---

## Current Implementation

The current daemon (`crates/agent_pool/src/daemon.rs`) mixes all three layers together.

### Current Data Structures

**Location:** `daemon.rs:329-340`
```rust
struct PoolState {
    agents_dir: PathBuf,           // I/O concern - doesn't belong in core state
    pending_dir: PathBuf,          // I/O concern - doesn't belong in core state
    agents: HashMap<String, AgentState>,
    pending: VecDeque<Task>,
    config: DaemonConfig,
}

struct Task {
    content: String,
    respond_to: ResponseTarget,    // I/O concern - mixes state with response routing
}
```

**Location:** `daemon.rs:239-260`
```rust
enum ResponseTarget {
    Socket(Stream),                // Holds actual I/O handle
    File(PathBuf),
}

enum AgentStatus {
    Idle,
    Busy(InFlight),
}

enum InFlight {
    Task { respond_to: ResponseTarget },  // I/O handle embedded in state
    HealthCheck,
}
```

**Location:** `daemon.rs:281-302`
```rust
struct AgentState {
    status: AgentStatus,
    last_activity: Instant,        // Time-based - makes testing harder
}
```

### Current Event Loop

**Location:** `daemon.rs:659-724`
```rust
fn event_loop(
    listener: &Listener,           // I/O
    fs_events: &mpsc::Receiver<Event>,  // I/O
    state: &mut PoolState,
    signals: &DaemonSignals,
) -> io::Result<()> {
    loop {
        // Check shutdown
        if signals.is_shutdown_triggered() { ... }

        // Socket I/O - mixed with state
        if let Some(task) = accept_task(listener)? {
            state.enqueue(task);
        }

        // FS event handling - mixed with state
        match fs_events.recv_timeout(poll_timeout) {
            Ok(event) => handle_fs_event(&event, state)?,
            ...
        }

        // Periodic scans - compensating for unreliable fs events
        if last_scan.elapsed() >= scan_interval {
            state.scan_agents()?;
            state.scan_outputs()?;
            state.scan_pending()?;
            state.check_periodic_health_checks()?;
            state.check_health_check_timeouts();
        }

        // Dispatch - business logic mixed with I/O
        if !signals.is_paused() {
            state.dispatch_pending()?;
        }
    }
}
```

### Problems with Current Design

1. **I/O handles in state:** `ResponseTarget::Socket(Stream)` embeds I/O in state
2. **Paths in state:** `agents_dir`, `pending_dir` are I/O concerns
3. **Time in state:** `Instant` makes testing non-deterministic
4. **Methods do I/O:** `dispatch_to()` calls `fs::write()` directly
5. **Mixed concerns:** `PoolState` methods do both state transitions AND I/O
6. **Hard to test:** Can't test state machine without filesystem/sockets

---

## Target Data Structures

### Layer 1: Pure State (no I/O, no time)

```rust
/// Unique identifier for a task.
/// Layer 3 maps this to content, responder, and task kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TaskId(u32);

impl From<u32> for TaskId {
    fn from(id: u32) -> Self {
        TaskId(id)
    }
}

/// Unique identifier for an agent.
/// Layer 3 maps this to the actual directory name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AgentId(u32);

impl From<u32> for AgentId {
    fn from(id: u32) -> Self {
        AgentId(id)
    }
}

/// Agent epoch - identifies a specific point in an agent's lifecycle.
/// Contains the agent_id so epochs from different agent registrations
/// can never accidentally match. Cheap to clone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Epoch {
    pub agent_id: AgentId,
    pub sequence: u32,
}

/// Pure pool state - only IDs, no content, no I/O, no time, no config
pub struct PoolState {
    /// Task IDs waiting to be assigned to agents.
    /// Just IDs - Layer 3 holds the actual content and responder info.
    pending_tasks: VecDeque<TaskId>,

    /// Registered agents and their current status.
    /// BTreeMap for deterministic iteration order (snapshots, debugging).
    agents: BTreeMap<AgentId, AgentState>,
}

/// Agent state - no I/O, no absolute time
pub struct AgentState {
    pub status: AgentStatus,
    /// Incremented on every state transition (idle→busy, busy→idle).
    /// Used to validate timeout events - stale timers have wrong epoch.
    pub epoch: Epoch,
}

pub enum AgentStatus {
    /// Ready to receive work
    Idle,
    /// Currently processing a task (submission or health check)
    /// Layer 3 knows whether it's a submission or health check.
    Busy { task_id: TaskId },
}

// No PoolConfig in Layer 1!
// Layer 1 is purely reactive - it processes events and emits effects.
// Layer 3 decides policy (whether to start timers, send health checks, etc.)
```

### Layer 1: Events (inputs to state machine)

```rust
/// All possible events that can affect state.
/// Named in past tense - these are things that HAPPENED.
/// Note: No content in events - Layer 3 holds all content.
/// Note: No time in Layer 1 - timeout events come from Layer 3 timers.
pub enum Event {
    /// A task was submitted (from socket or file).
    /// Layer 3 has already stored the content and responder.
    TaskSubmitted {
        task_id: TaskId,
    },

    /// A task was withdrawn (submitter disconnected/died before completion).
    /// Remove from pending queue if not yet dispatched.
    /// If already dispatched, the response will be discarded.
    TaskWithdrawn {
        task_id: TaskId,
    },

    /// An agent directory appeared (agent registered).
    /// Layer 3 assigns the AgentId and maintains the name mapping.
    AgentRegistered {
        agent_id: AgentId,
    },

    /// An agent directory was removed (agent deregistered).
    AgentDeregistered {
        agent_id: AgentId,
    },

    /// An agent wrote response.json (task completed).
    /// Layer 3 stores the response content; Layer 1 just knows the agent responded.
    AgentResponded {
        agent_id: AgentId,
    },

    /// An agent timeout fired (from Layer 3 timer).
    /// Layer 1 checks if agent still exists with matching epoch.
    /// If yes: deregister agent (and TaskFailed if busy).
    /// If no: epoch changed, ignore stale timeout.
    ///
    /// Agents that are still alive will call get_task again and re-register.
    AgentTimedOut {
        epoch: Epoch,  // Contains agent_id
    },

    // TODO: Implement pause/resume/shutdown
    // PauseRequested - stop dispatching new tasks, let in-flight complete
    // ResumeRequested - resume dispatching
    // ShutdownRequested - drain in-flight tasks, then emit ShutdownComplete
}
```

### Layer 1: Effects (outputs from state machine)

```rust
/// Actions for Layer 3 to execute.
/// Note: Minimal data - Layer 3 looks up content/responder from IDs.
pub enum Effect {
    /// Dispatch a submitted task to an agent.
    /// Layer 3 looks up: agent directory name, task content from task_id.
    /// Layer 3 also starts a task timeout timer using the epoch.
    DispatchTask {
        task_id: TaskId,
        epoch: Epoch,  // Contains agent_id
    },

    /// Agent became idle (after registration or task completion).
    /// Layer 3 starts a timeout timer - if agent doesn't get work,
    /// they'll be deregistered. Alive agents will re-register.
    AgentBecameIdle {
        epoch: Epoch,  // Contains agent_id
    },

    /// Task completed successfully - send response to submitter (if any).
    /// For submissions: Layer 3 looks up responder and pending response, sends it.
    /// For health checks: Layer 3 has no responder, just cleans up.
    /// agent_id lets Layer 3 look up the pending response content.
    TaskCompleted {
        agent_id: AgentId,
        task_id: TaskId,
    },

    /// Task failed (agent timed out) - send error to submitter (if any).
    /// For submissions: Layer 3 looks up responder, sends error.
    /// For health checks: Layer 3 has no responder, nothing to do.
    TaskFailed {
        task_id: TaskId,
    },

    /// Deregister agent (remove directory due to timeout).
    /// Layer 3 looks up the directory name from AgentId.
    DeregisterAgent {
        agent_id: AgentId,
    },

    // TODO: ShutdownComplete - signal that shutdown is done
}

// No Log effect - Layer 2/3 should automatically log all events and effects
```

### Layer 1: Pure Step Function

```rust
/// Pure state transition - no I/O, fully deterministic
pub fn step(state: PoolState, event: Event) -> (PoolState, Vec<Effect>) {
    match event {
        Event::TaskSubmitted { task_id } => {
            handle_task_submitted(state, task_id)
        }
        Event::TaskWithdrawn { task_id } => {
            handle_task_withdrawn(state, task_id)
        }
        Event::AgentRegistered { agent_id } => {
            handle_agent_registered(state, agent_id)
        }
        Event::AgentDeregistered { agent_id } => {
            handle_agent_deregistered(state, agent_id)
        }
        Event::AgentResponded { agent_id } => {
            handle_agent_responded(state, agent_id)
        }
        Event::AgentTimedOut { epoch } => {
            handle_agent_timed_out(state, epoch)
        }
        // TODO: Implement pause/resume/shutdown handlers
    }
}

fn handle_task_submitted(
    mut state: PoolState,
    task_id: TaskId,
) -> (PoolState, Vec<Effect>) {
    // Just queue the ID - Layer 3 holds the content and responder
    state.pending_tasks.push_back(task_id);

    let effects = try_dispatch(&mut state);
    (state, effects)
}

fn handle_task_withdrawn(
    mut state: PoolState,
    task_id: TaskId,
) -> (PoolState, Vec<Effect>) {
    // Remove from pending queue if not yet dispatched
    state.pending_tasks.retain(|&id| id != task_id);

    // If task was already dispatched to an agent, we can't recall it.
    // When the agent responds, TaskCompleted will be emitted, but
    // Layer 3 will find no responder (it was cleaned up) and discard the response.

    (state, vec![])
}

fn handle_agent_registered(
    mut state: PoolState,
    agent_id: AgentId,
) -> (PoolState, Vec<Effect>) {
    if state.agents.contains_key(&agent_id) {
        return (state, vec![]);
    }

    let epoch = Epoch { agent_id, sequence: 0 };
    state.agents.insert(agent_id, AgentState {
        status: AgentStatus::Idle,
        epoch,
    });

    let mut effects = vec![
        Effect::AgentBecameIdle { epoch },
    ];

    // Try to dispatch pending tasks to this new agent
    effects.extend(try_dispatch(&mut state));

    (state, effects)
}

fn handle_agent_responded(
    mut state: PoolState,
    agent_id: AgentId,
) -> (PoolState, Vec<Effect>) {
    let Some(agent) = state.agents.get_mut(&agent_id) else {
        return (state, vec![]);
    };

    let AgentStatus::Busy { task_id } = std::mem::replace(
        &mut agent.status,
        AgentStatus::Idle,
    ) else {
        return (state, vec![]);
    };

    // Increment epoch on state transition - invalidates any pending timeout
    agent.epoch.sequence += 1;
    let new_epoch = agent.epoch;

    // Layer 3 looks up response content using agent_id
    let mut effects = vec![
        Effect::TaskCompleted { agent_id, task_id },
        Effect::AgentBecameIdle { epoch: new_epoch },
    ];

    // Try to dispatch more tasks
    effects.extend(try_dispatch(&mut state));

    (state, effects)
}

fn handle_agent_timed_out(
    mut state: PoolState,
    epoch: Epoch,
) -> (PoolState, Vec<Effect>) {
    let agent_id = epoch.agent_id;

    let Some(agent) = state.agents.get(&agent_id) else {
        // Agent already removed, ignore
        return (state, vec![]);
    };

    if agent.epoch != epoch {
        // Epoch mismatch - agent did work since timer started, ignore stale timeout
        return (state, vec![]);
    }

    // Capture task_id if agent was busy (for TaskFailed effect)
    let in_flight_task = match agent.status {
        AgentStatus::Busy { task_id } => Some(task_id),
        AgentStatus::Idle => None,
    };

    // Deregister the agent
    state.agents.remove(&agent_id);

    let mut effects = vec![Effect::DeregisterAgent { agent_id }];

    // If agent was busy, the task failed
    if let Some(task_id) = in_flight_task {
        effects.insert(0, Effect::TaskFailed { task_id });
    }

    (state, effects)
}

fn try_dispatch(state: &mut PoolState) -> Vec<Effect> {
    let mut effects = vec![];

    while let Some(agent_id) = find_idle_agent(state) {
        let Some(task_id) = state.pending_tasks.pop_front() else {
            break;
        };

        let agent = state.agents.get_mut(&agent_id)
            .expect("find_idle_agent returned agent not in state");
        // Increment epoch on state transition (idle → busy)
        agent.epoch.sequence += 1;
        agent.status = AgentStatus::Busy { task_id };

        effects.push(Effect::DispatchTask { task_id, epoch: agent.epoch });
    }

    effects
}

fn find_idle_agent(state: &PoolState) -> Option<AgentId> {
    state.agents
        .iter()
        .find(|(_, a)| matches!(a.status, AgentStatus::Idle))
        .map(|(id, _)| *id)
}

```

### Layer 2: Event Loop

```rust
/// Orchestrates Layer 1 and Layer 3
pub fn event_loop(
    mut state: PoolState,
    events_rx: mpsc::Receiver<Event>,
    effects_tx: mpsc::Sender<Effect>,
) {
    while let Ok(event) = events_rx.recv() {
        // Log all incoming events
        tracing::debug!(?event, "received event");

        let (new_state, effects) = step(state, event);
        state = new_state;

        for effect in effects {
            // Log all outgoing effects
            tracing::debug!(?effect, "emitting effect");

            if matches!(effect, Effect::ShutdownComplete) {
                return;
            }
            let _ = effects_tx.send(effect);
        }
    }
}
```

### Layer 3: I/O

```rust
/// Communication channel - used for both agents and submissions.
/// Identical protocol: directory-based or socket-based.
enum Channel {
    /// Filesystem-based: directory containing task.json/response.json
    Directory(PathBuf),
    // TODO: Socket-based communication (future refactor)
    // Socket(Stream),
}

impl Channel {
    /// Write content to a file in this channel.
    fn write(&self, filename: &str, content: &str) -> io::Result<()> {
        match self {
            Channel::Directory(path) => {
                fs::write(path.join(filename), content)
            }
            // Channel::Socket(stream) => { ... }
        }
    }

    /// Clean up this channel (remove directory, close socket, etc.)
    fn cleanup(&self) {
        match self {
            Channel::Directory(path) => {
                let _ = fs::remove_dir_all(path);
            }
            // Channel::Socket(_) => { /* socket closes on drop */ }
        }
    }
}

/// Generic map for channels keyed by ID.
/// Used for both agents (AgentId) and submissions (TaskId).
/// ID type must be constructible from u32.
struct ChannelMap<Id> {
    channels: HashMap<Id, Channel>,
    /// For deduplicating FS events - tracks which paths are registered
    path_to_id: HashMap<PathBuf, Id>,
    next_id: u32,
}

impl<Id: From<u32> + Copy + Eq + std::hash::Hash> ChannelMap<Id> {
    fn new() -> Self {
        Self {
            channels: HashMap::new(),
            path_to_id: HashMap::new(),
            next_id: 0,
        }
    }

    fn next_id(&mut self) -> Id {
        let id = Id::from(self.next_id);
        self.next_id += 1;
        id
    }

    /// Register a directory-based channel. Returns None if path already registered.
    fn register_directory(&mut self, path: PathBuf) -> Option<Id> {
        if self.path_to_id.contains_key(&path) {
            return None;  // Duplicate FS event, ignore
        }
        let id = self.next_id();
        self.path_to_id.insert(path.clone(), id);
        self.channels.insert(id, Channel::Directory(path));
        Some(id)
    }

    // TODO: Socket-based registration (future refactor)
    // fn register_socket(&mut self, socket: Stream) -> Id {
    //     // Sockets are always unique - no deduplication needed
    //     let id = self.next_id();
    //     self.channels.insert(id, Channel::Socket(socket));
    //     id
    // }

    fn get(&self, id: Id) -> Option<&Channel> {
        self.channels.get(&id)
    }

    fn remove(&mut self, id: Id) {
        let channel = self.channels.remove(&id)
            .expect("remove() called for unknown Id - Layer 1 bug");
        // Clean up path_to_id for directory-based channels
        if let Channel::Directory(ref path) = channel {
            self.path_to_id.remove(path);
        }
        // Clean up the channel itself (delete directory, close socket, etc.)
        channel.cleanup();
    }
}

/// Agent map is just a ChannelMap keyed by AgentId.
type AgentMap = ChannelMap<AgentId>;

/// Task map extends ChannelMap with content storage.
/// Submissions have content (the task) plus a channel (to respond on).
struct TaskMap {
    channels: ChannelMap<TaskId>,
    content: HashMap<TaskId, String>,
}

impl TaskMap {
    fn new() -> Self {
        Self {
            channels: ChannelMap::new(),
            content: HashMap::new(),
        }
    }

    /// Register a directory-based submission. Returns None if path already registered.
    fn register_directory(&mut self, path: PathBuf, task_content: String) -> Option<TaskId> {
        let id = self.channels.register_directory(path)?;
        self.content.insert(id, task_content);
        Some(id)
    }

    // TODO: Socket-based registration (future refactor)
    // fn register_socket(&mut self, socket: Stream, task_content: String) -> TaskId {
    //     let id = self.channels.register_socket(socket);
    //     self.content.insert(id, task_content);
    //     id
    // }

    fn get_content(&self, id: TaskId) -> &str {
        self.content.get(&id)
            .expect("get_content for unknown TaskId")
    }

    fn get_channel(&self, id: TaskId) -> Option<&Channel> {
        self.channels.get(id)
    }

    /// Complete a task: send response via channel, then clean up.
    fn complete(&mut self, id: TaskId, response: &str) -> io::Result<()> {
        self.content.remove(&id);
        let channel = self.channels.channels.remove(&id)
            .expect("complete() for unknown TaskId");
        // Clean up path_to_id for directory-based channels
        if let Channel::Directory(ref path) = channel {
            self.channels.path_to_id.remove(path);
        }
        // Send response (don't cleanup directory - submitter needs to read it)
        channel.write("response.json", response)
    }

    /// Fail a task: send error via channel, then clean up.
    fn fail(&mut self, id: TaskId, error: &str) -> io::Result<()> {
        self.complete(id, error)
    }
}

/// Layer 3 configuration - all policy decisions live here, not in Layer 1
pub struct IoConfig {
    /// How long before an agent times out (busy or idle).
    /// Busy agents: task fails, agent deregistered.
    /// Idle agents: agent deregistered.
    /// Alive agents will re-register by calling get_task again.
    pub agent_timeout: Duration,
}

/// Layer 3: All I/O happens here
pub fn io_layer(
    events_tx: mpsc::Sender<Event>,
    effects_rx: mpsc::Receiver<Effect>,
    listener: Listener,
    fs_events: mpsc::Receiver<notify::Event>,
    agents_dir: PathBuf,
    pending_dir: PathBuf,
    config: IoConfig,
) -> io::Result<()> {
    let mut agent_map = AgentMap::new();
    let mut task_map = TaskMap::new();
    // Pending responses: when agent writes response.json, we store content here
    // until TaskCompleted effect tells us to send it
    let mut pending_responses: HashMap<AgentId, String> = HashMap::new();

    // No periodic TimerTick - timers are started on-demand via effects

    // Main I/O loop
    loop {
        // Non-blocking socket accept
        if let Ok(stream) = listener.accept() {
            if let Some((content, stream)) = read_submission(stream) {
                // Register submission in TaskMap, get TaskId
                let task_id = task_map.register_submission(content, Responder::Socket(stream));
                let _ = events_tx.send(Event::TaskSubmitted { task_id });
            }
        }

        // FS events (non-blocking)
        while let Ok(event) = fs_events.try_recv() {
            for parsed in parse_fs_event(&event, &agents_dir, &pending_dir, &mut agent_map, &mut task_map) {
                let _ = events_tx.send(parsed);
            }
        }

        // Execute effects (non-blocking)
        while let Ok(effect) = effects_rx.try_recv() {
            execute_effect(effect, &mut agent_map, &mut task_map, &mut pending_responses, &events_tx, &config)?;
        }

        thread::sleep(Duration::from_millis(10));
    }
}

fn execute_effect(
    effect: Effect,
    agent_map: &mut AgentMap,
    task_map: &mut TaskMap,
    pending_responses: &mut HashMap<AgentId, String>,
    events_tx: &mpsc::Sender<Event>,
    config: &IoConfig,
) -> io::Result<()> {
    match effect {
        Effect::DispatchTask { task_id, epoch } => {
            let channel = agent_map.get(epoch.agent_id)
                .expect("DispatchTask for unknown agent - Layer 1 bug");
            let content = task_map.get_content(task_id);
            let envelope = serde_json::json!({
                "kind": "Task",
                "content": serde_json::from_str::<serde_json::Value>(content)
                    .unwrap_or(serde_json::Value::String(content.to_string())),
            }).to_string();
            channel.write("task.json", &envelope)?;

            // Start timeout timer
            start_timeout_timer(events_tx.clone(), epoch, config.agent_timeout);
        }
        Effect::AgentBecameIdle { epoch } => {
            // Start timeout timer - if agent doesn't get work, they'll be deregistered
            start_timeout_timer(events_tx.clone(), epoch, config.agent_timeout);
        }
        Effect::TaskCompleted { agent_id, task_id } => {
            let response_content = pending_responses.remove(&agent_id)
                .expect("TaskCompleted for agent with no pending response - Layer 3 bug");
            task_map.complete(task_id, &response_content)?;
        }
        Effect::TaskFailed { task_id } => {
            let error = serde_json::json!({
                "status": "NotProcessed",
                "reason": "AgentTimeout"
            }).to_string();
            task_map.fail(task_id, &error)?;
        }
        Effect::DeregisterAgent { agent_id } => {
            // remove() cleans up internal maps and the channel itself
            agent_map.remove(agent_id);
        }
        // TODO: Handle ShutdownComplete
    }
    Ok(())
}

/// Start a timer that sends AgentTimedOut after the given duration.
/// The timer is "fire and forget" - Layer 1 ignores it if epoch doesn't match.
fn start_timeout_timer(
    events_tx: mpsc::Sender<Event>,
    epoch: Epoch,
    timeout: Duration,
) {
    thread::spawn(move || {
        thread::sleep(timeout);
        let _ = events_tx.send(Event::AgentTimedOut { epoch });
    });
}
```

---

## Migration Tasks

### Phase 1: Define Types

| Status | Task |
|--------|------|
| [ ] | Create `crates/agent_pool/src/core.rs` with pure types |
| [ ] | Define `PoolState`, `AgentState`, `AgentStatus`, `TaskKind` |
| [ ] | Define `Event` enum |
| [ ] | Define `Effect` enum |
| [ ] | Define `ResponderId`, `TaskId` |
| [ ] | Add unit tests for type construction |

### Phase 2: Implement Pure Step Function

| Status | Task |
|--------|------|
| [ ] | Implement `step(state, event) -> (state, effects)` |
| [ ] | Implement `handle_submission()` |
| [ ] | Implement `handle_agent_registered()` |
| [ ] | Implement `handle_agent_deregistered()` |
| [ ] | Implement `handle_agent_response()` |
| [ ] | Implement `handle_timer_tick()` |
| [ ] | Implement `try_dispatch()` |
| [ ] | Implement `dispatch_health_check()` |
| [ ] | Add comprehensive unit tests for step function |

### Phase 3: Create Layer 2 Event Loop

| Status | Task |
|--------|------|
| [ ] | Create thin event loop that calls `step()` |
| [ ] | Wire up channels for events and effects |
| [ ] | Test with mock events |

### Phase 4: Refactor Layer 3 (I/O)

| Status | Task |
|--------|------|
| [ ] | Create `ResponderMap` for response routing |
| [ ] | Move socket accept logic to Layer 3 |
| [ ] | Move fs event parsing to Layer 3 |
| [ ] | Move effect execution to Layer 3 |
| [ ] | Remove I/O from `PoolState` methods |

### Phase 5: Integration

| Status | Task |
|--------|------|
| [ ] | Wire all three layers together |
| [ ] | Run existing integration tests |
| [ ] | Remove old mixed implementation |
| [ ] | Clean up dead code |

---

## Testing Strategy

### Layer 1 Tests (Pure)

```rust
#[test]
fn test_task_submitted_queues_task() {
    let state = PoolState::new();
    let task_id = TaskId(1);

    let (state, effects) = step(state, Event::TaskSubmitted { task_id });

    assert_eq!(state.pending_tasks.len(), 1);
    assert_eq!(state.pending_tasks[0], task_id);
    assert!(effects.is_empty()); // No agent to dispatch to
}

#[test]
fn test_task_submitted_dispatches_to_idle_agent() {
    let mut state = PoolState::new();
    let agent_id = AgentId(1);
    state.agents.insert(agent_id, AgentState {
        status: AgentStatus::Idle,
        epoch: Epoch { agent_id, sequence: 0 },
    });
    let task_id = TaskId(42);

    let (state, effects) = step(state, Event::TaskSubmitted { task_id });

    assert!(state.pending_tasks.is_empty());
    // Agent now busy, epoch sequence incremented to 1
    let agent = state.agents.get(&agent_id).unwrap();
    assert!(matches!(agent.status, AgentStatus::Busy { task_id: t } if t == task_id));
    assert_eq!(agent.epoch.sequence, 1);
    // DispatchTask includes epoch for timeout timer
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::DispatchTask { task_id: t, epoch }
            if *t == task_id && epoch.agent_id == agent_id
    )));
}

#[test]
fn test_task_withdrawn_removes_from_queue() {
    let mut state = PoolState::new();
    state.pending_tasks.push_back(TaskId(1));
    state.pending_tasks.push_back(TaskId(2));
    state.pending_tasks.push_back(TaskId(3));

    let (state, _effects) = step(state, Event::TaskWithdrawn { task_id: TaskId(2) });

    assert_eq!(state.pending_tasks.len(), 2);
    assert!(!state.pending_tasks.contains(&TaskId(2)));
}

#[test]
fn test_agent_responded_completes_task_and_increments_epoch() {
    let mut state = PoolState::new();
    let agent_id = AgentId(1);
    let task_id = TaskId(42);
    state.agents.insert(agent_id, AgentState {
        status: AgentStatus::Busy { task_id },
        epoch: Epoch { agent_id, sequence: 5 },
    });

    let (state, effects) = step(state, Event::AgentResponded { agent_id });

    // Agent is now idle with incremented epoch
    let agent = state.agents.get(&agent_id).unwrap();
    assert!(matches!(agent.status, AgentStatus::Idle));
    assert_eq!(agent.epoch.sequence, 6); // Incremented from 5 to 6

    // TaskCompleted and AgentBecameIdle effects emitted
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::TaskCompleted { agent_id: a, task_id: t }
            if *a == agent_id && *t == task_id
    )));
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::AgentBecameIdle { epoch } if epoch.agent_id == agent_id && epoch.sequence == 6
    )));
}

#[test]
fn test_busy_agent_timed_out_deregisters_and_fails_task() {
    let mut state = PoolState::new();
    let agent_id = AgentId(1);
    let task_id = TaskId(42);
    let epoch = Epoch { agent_id, sequence: 7 };
    state.agents.insert(agent_id, AgentState {
        status: AgentStatus::Busy { task_id },
        epoch,
    });

    let (state, effects) = step(state, Event::AgentTimedOut { epoch });

    // Agent removed
    assert!(!state.agents.contains_key(&agent_id));
    // TaskFailed and DeregisterAgent emitted
    assert!(effects.iter().any(|e| matches!(e, Effect::TaskFailed { task_id: t } if *t == task_id)));
    assert!(effects.iter().any(|e| matches!(e, Effect::DeregisterAgent { agent_id: a } if *a == agent_id)));
}

#[test]
fn test_idle_agent_timed_out_deregisters() {
    let mut state = PoolState::new();
    let agent_id = AgentId(1);
    let epoch = Epoch { agent_id, sequence: 4 };
    state.agents.insert(agent_id, AgentState {
        status: AgentStatus::Idle,
        epoch,
    });

    let (state, effects) = step(state, Event::AgentTimedOut { epoch });

    // Agent removed
    assert!(!state.agents.contains_key(&agent_id));
    // DeregisterAgent emitted (no TaskFailed since agent was idle)
    assert!(effects.iter().any(|e| matches!(e, Effect::DeregisterAgent { agent_id: a } if *a == agent_id)));
    assert!(!effects.iter().any(|e| matches!(e, Effect::TaskFailed { .. })));
}

#[test]
fn test_stale_timeout_ignored() {
    let mut state = PoolState::new();
    let agent_id = AgentId(1);
    // Agent has epoch sequence 5, but timeout is for sequence 3 (stale)
    state.agents.insert(agent_id, AgentState {
        status: AgentStatus::Busy { task_id: TaskId(99) },
        epoch: Epoch { agent_id, sequence: 5 },
    });

    let stale_epoch = Epoch { agent_id, sequence: 3 };
    let (state, effects) = step(state, Event::AgentTimedOut { epoch: stale_epoch });

    // Agent still exists - stale timeout ignored
    assert!(state.agents.contains_key(&agent_id));
    assert!(effects.is_empty());
}
```

These tests run instantly with no I/O, no threads, no timing issues. Note that:
- Layer 1 doesn't know about content or time - it just routes IDs
- Timeout events are "soft" - Layer 1 validates them against current state
- Stale timeouts (wrong task_id or epoch) are silently ignored
- Tests don't need to set up any mocks for I/O, timers, or content storage

---

## File Structure

Each layer should be in a separate file to enforce boundaries via Rust's visibility rules:

```
crates/agent_pool/src/
├── lib.rs              # Public API
├── core.rs             # Layer 1: Pure state machine
│   ├── PoolState, AgentState, AgentStatus
│   ├── Event, Effect, TaskKind
│   ├── AgentId, TaskId, ResponderId
│   └── fn step(state, event) -> (state, effects)
├── event_loop.rs       # Layer 2: Orchestration
│   └── fn event_loop(state, events_rx, effects_tx)
├── io.rs               # Layer 3: I/O
│   ├── ResponderMap, AgentMap
│   ├── fn io_layer(...)
│   └── fn execute_effect(...)
└── daemon.rs           # Wiring: spawns threads, connects layers
```

**Benefits:**
- `core.rs` has no `use std::fs`, no `use std::net` - enforced at module level
- Layer 1 types are `pub`, Layer 3 types are `pub(crate)` or private
- Can't accidentally add I/O to Layer 1 without changing imports

---

## State Ownership Summary

| Data | Layer | Reason |
|------|-------|--------|
| `pending_tasks: VecDeque<TaskId>` | 1 | Core queue logic (just IDs) |
| `agents: BTreeMap<AgentId, AgentState>` | 1 | Core dispatch logic |
| `IoConfig` | 3 | All policy (timeouts) |
| `Channel` | 3 | Unified communication channel (Directory or Socket) |
| `ChannelMap<Id>` | 3 | Generic map from ID to Channel with deduplication |
| `AgentMap` = `ChannelMap<AgentId>` | 3 | Maps AgentId to Channel |
| `TaskMap` | 3 | Extends ChannelMap with content storage |
| `pending_responses` | 3 | Temporary storage for agent response content |
| `Listener` | 3 | Socket I/O for submissions |
| `Watcher` | 3 | FS I/O |

**Key insights:**
- **Layer 1 has no config** - it's purely reactive, processing events and emitting effects
- **Layer 1 only knows about IDs** (AgentId, TaskId) - Layer 3 maps IDs to actual content and channels
- **Layer 1 has no concept of time** - timeout events come from Layer 3 timers
- **Epochs validate timeouts** - stale timers (wrong epoch) are silently ignored
- **Agents are anonymous** - no "names", just unique AgentIds and communication channels
- **Unified Channel type** - both agents and submissions use the same Channel enum
- **Generic ChannelMap** - AgentMap and TaskMap share the same base implementation

---

## TODO

- [ ] Extract architectural principles (serial event processing, Byzantine resilience, three-layer separation, IDs vs handles) to a prominent document (README.md or CLAUDE.md) for project-wide guidance.
