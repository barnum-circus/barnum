# Three-Layer Daemon Refactor

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
/// Unique identifier for a pending response.
/// Layer 3 maps this back to the actual socket/file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResponderId(u64);

/// Unique identifier for a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TaskId(u64);

/// Pure pool state - no I/O handles, no paths, no time
pub struct PoolState {
    /// Tasks waiting to be assigned to agents
    pending_tasks: VecDeque<PendingTask>,

    /// Registered agents and their current status
    agents: HashMap<String, AgentState>,

    /// Counter for generating unique IDs
    next_id: u64,

    /// Configuration (immutable after creation)
    config: PoolConfig,
}

/// A task waiting in the queue
pub struct PendingTask {
    pub id: TaskId,
    pub content: String,
    pub responder: ResponderId,
}

/// Agent state - no I/O, no absolute time
pub struct AgentState {
    pub status: AgentStatus,
    /// Ticks since last activity (incremented by TimerTick event)
    pub idle_ticks: u32,
}

pub enum AgentStatus {
    /// Ready to receive work
    Idle,
    /// Currently processing a task
    Busy { task_id: TaskId, kind: TaskKind },
}

pub enum TaskKind {
    /// Regular task from a submitter
    Submission { responder: ResponderId },
    /// Health check ping
    HealthCheck,
}

/// Configuration - all durations expressed as tick counts
pub struct PoolConfig {
    pub initial_health_check: bool,
    pub periodic_health_check: bool,
    /// How many ticks before sending health check to idle agent
    pub health_check_interval_ticks: u32,
    /// How many ticks before timing out a health check
    pub health_check_timeout_ticks: u32,
}
```

### Layer 1: Events (inputs to state machine)

```rust
/// All possible events that can affect state
pub enum Event {
    /// New task submitted (from socket or file)
    Submission {
        content: String,
        responder: ResponderId,
    },

    /// Agent directory appeared (agent registered)
    AgentRegistered {
        agent_id: String,
    },

    /// Agent directory removed (agent deregistered)
    AgentDeregistered {
        agent_id: String,
    },

    /// Agent wrote response.json (task completed)
    AgentResponse {
        agent_id: String,
        response: String,
    },

    /// Timer tick (for health check scheduling/timeouts)
    /// Layer 2 sends this periodically (e.g., every 1 second)
    TimerTick,

    /// External request to pause dispatching
    Pause,

    /// External request to resume dispatching
    Resume,

    /// External request to shut down
    Shutdown,
}
```

### Layer 1: Effects (outputs from state machine)

```rust
/// Actions for Layer 3 to execute
pub enum Effect {
    /// Send response to a submitter
    SendResponse {
        responder: ResponderId,
        content: String,
    },

    /// Dispatch task to an agent (write task.json)
    DispatchTask {
        agent_id: String,
        task_id: TaskId,
        envelope: String,  // JSON envelope with kind + content
    },

    /// Deregister agent (remove directory due to timeout)
    DeregisterAgent {
        agent_id: String,
    },

    /// Log a message (for observability)
    Log {
        level: LogLevel,
        message: String,
    },

    /// Signal shutdown complete
    ShutdownComplete,
}

pub enum LogLevel {
    Debug,
    Info,
    Warn,
}
```

### Layer 1: Pure Step Function

```rust
/// Pure state transition - no I/O, fully deterministic
pub fn step(state: PoolState, event: Event) -> (PoolState, Vec<Effect>) {
    match event {
        Event::Submission { content, responder } => {
            handle_submission(state, content, responder)
        }
        Event::AgentRegistered { agent_id } => {
            handle_agent_registered(state, agent_id)
        }
        Event::AgentDeregistered { agent_id } => {
            handle_agent_deregistered(state, agent_id)
        }
        Event::AgentResponse { agent_id, response } => {
            handle_agent_response(state, agent_id, response)
        }
        Event::TimerTick => {
            handle_timer_tick(state)
        }
        Event::Pause => {
            // Could add `paused: bool` to state if needed
            (state, vec![])
        }
        Event::Resume => {
            (state, vec![])
        }
        Event::Shutdown => {
            handle_shutdown(state)
        }
    }
}

fn handle_submission(
    mut state: PoolState,
    content: String,
    responder: ResponderId,
) -> (PoolState, Vec<Effect>) {
    let task_id = state.next_task_id();
    let task = PendingTask { id: task_id, content, responder };
    state.pending_tasks.push_back(task);

    let effects = try_dispatch(&mut state);
    (state, effects)
}

fn handle_agent_registered(
    mut state: PoolState,
    agent_id: String,
) -> (PoolState, Vec<Effect>) {
    if state.agents.contains_key(&agent_id) {
        return (state, vec![]);
    }

    state.agents.insert(agent_id.clone(), AgentState::new());

    let mut effects = vec![Effect::Log {
        level: LogLevel::Info,
        message: format!("agent registered: {agent_id}"),
    }];

    if state.config.initial_health_check {
        if let Some(dispatch_effect) = dispatch_health_check(&mut state, &agent_id) {
            effects.push(dispatch_effect);
        }
    }

    // Try to dispatch pending tasks
    effects.extend(try_dispatch(&mut state));

    (state, effects)
}

fn handle_agent_response(
    mut state: PoolState,
    agent_id: String,
    response: String,
) -> (PoolState, Vec<Effect>) {
    let Some(agent) = state.agents.get_mut(&agent_id) else {
        return (state, vec![]);
    };

    let AgentStatus::Busy { task_id, kind } = std::mem::replace(
        &mut agent.status,
        AgentStatus::Idle,
    ) else {
        return (state, vec![]);
    };

    agent.idle_ticks = 0;

    let mut effects = vec![];

    match kind {
        TaskKind::Submission { responder } => {
            effects.push(Effect::SendResponse {
                responder,
                content: response,
            });
            effects.push(Effect::Log {
                level: LogLevel::Info,
                message: format!("task completed: agent={agent_id}"),
            });
        }
        TaskKind::HealthCheck => {
            effects.push(Effect::Log {
                level: LogLevel::Debug,
                message: format!("health check completed: agent={agent_id}"),
            });
        }
    }

    // Try to dispatch more tasks
    effects.extend(try_dispatch(&mut state));

    (state, effects)
}

fn handle_timer_tick(mut state: PoolState) -> (PoolState, Vec<Effect>) {
    let mut effects = vec![];

    // Increment idle ticks for all idle agents
    for agent in state.agents.values_mut() {
        if matches!(agent.status, AgentStatus::Idle) {
            agent.idle_ticks += 1;
        }
    }

    // Check for health check timeouts
    let timed_out: Vec<String> = state.agents
        .iter()
        .filter(|(_, a)| {
            matches!(a.status, AgentStatus::Busy { kind: TaskKind::HealthCheck, .. })
                && a.idle_ticks >= state.config.health_check_timeout_ticks
        })
        .map(|(id, _)| id.clone())
        .collect();

    for agent_id in timed_out {
        state.agents.remove(&agent_id);
        effects.push(Effect::DeregisterAgent { agent_id: agent_id.clone() });
        effects.push(Effect::Log {
            level: LogLevel::Warn,
            message: format!("health check timeout, deregistering: {agent_id}"),
        });
    }

    // Send periodic health checks to stale idle agents
    if state.config.periodic_health_check {
        let stale: Vec<String> = state.agents
            .iter()
            .filter(|(_, a)| {
                matches!(a.status, AgentStatus::Idle)
                    && a.idle_ticks >= state.config.health_check_interval_ticks
            })
            .map(|(id, _)| id.clone())
            .collect();

        for agent_id in stale {
            if let Some(effect) = dispatch_health_check(&mut state, &agent_id) {
                effects.push(effect);
            }
        }
    }

    (state, effects)
}

fn try_dispatch(state: &mut PoolState) -> Vec<Effect> {
    let mut effects = vec![];

    while let Some(agent_id) = find_idle_agent(state) {
        let Some(task) = state.pending_tasks.pop_front() else {
            break;
        };

        let envelope = serde_json::json!({
            "kind": "Task",
            "content": task.content,
        }).to_string();

        if let Some(agent) = state.agents.get_mut(&agent_id) {
            agent.status = AgentStatus::Busy {
                task_id: task.id,
                kind: TaskKind::Submission { responder: task.responder },
            };
            agent.idle_ticks = 0;
        }

        effects.push(Effect::DispatchTask {
            agent_id: agent_id.clone(),
            task_id: task.id,
            envelope,
        });
        effects.push(Effect::Log {
            level: LogLevel::Info,
            message: format!("task dispatched: agent={agent_id}"),
        });
    }

    effects
}

fn find_idle_agent(state: &PoolState) -> Option<String> {
    state.agents
        .iter()
        .find(|(_, a)| matches!(a.status, AgentStatus::Idle))
        .map(|(id, _)| id.clone())
}

fn dispatch_health_check(state: &mut PoolState, agent_id: &str) -> Option<Effect> {
    let agent = state.agents.get_mut(agent_id)?;

    if !matches!(agent.status, AgentStatus::Idle) {
        return None;
    }

    let task_id = state.next_task_id();
    agent.status = AgentStatus::Busy {
        task_id,
        kind: TaskKind::HealthCheck,
    };
    agent.idle_ticks = 0;

    let envelope = serde_json::json!({
        "kind": "HealthCheck",
        "content": { "instructions": "Respond with any value to confirm you are alive." }
    }).to_string();

    Some(Effect::DispatchTask {
        agent_id: agent_id.to_string(),
        task_id,
        envelope,
    })
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
        let (new_state, effects) = step(state, event);
        state = new_state;

        for effect in effects {
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
/// Maps ResponderId back to actual response mechanism
struct ResponderMap {
    map: HashMap<ResponderId, Responder>,
    next_id: u64,
}

enum Responder {
    Socket(Stream),
    File(PathBuf),
}

impl ResponderMap {
    fn register_socket(&mut self, stream: Stream) -> ResponderId {
        let id = ResponderId(self.next_id);
        self.next_id += 1;
        self.map.insert(id, Responder::Socket(stream));
        id
    }

    fn register_file(&mut self, path: PathBuf) -> ResponderId {
        let id = ResponderId(self.next_id);
        self.next_id += 1;
        self.map.insert(id, Responder::File(path));
        id
    }

    fn send(&mut self, id: ResponderId, content: &str) -> io::Result<()> {
        let Some(responder) = self.map.remove(&id) else {
            return Ok(());
        };
        match responder {
            Responder::Socket(mut stream) => {
                writeln!(stream, "{}", content.len())?;
                stream.write_all(content.as_bytes())?;
                stream.flush()
            }
            Responder::File(path) => {
                fs::write(&path, content)
            }
        }
    }
}

/// Layer 3: All I/O happens here
pub fn io_layer(
    events_tx: mpsc::Sender<Event>,
    effects_rx: mpsc::Receiver<Effect>,
    listener: Listener,
    fs_events: mpsc::Receiver<notify::Event>,
    agents_dir: PathBuf,
    pending_dir: PathBuf,
) -> io::Result<()> {
    let mut responders = ResponderMap::new();

    // Timer thread sends TimerTick every second
    let events_tx_timer = events_tx.clone();
    thread::spawn(move || {
        loop {
            thread::sleep(Duration::from_secs(1));
            if events_tx_timer.send(Event::TimerTick).is_err() {
                break;
            }
        }
    });

    // Main I/O loop
    loop {
        // Non-blocking socket accept
        if let Ok(stream) = listener.accept() {
            if let Some((content, stream)) = read_submission(stream) {
                let responder = responders.register_socket(stream);
                let _ = events_tx.send(Event::Submission { content, responder });
            }
        }

        // FS events (non-blocking)
        while let Ok(event) = fs_events.try_recv() {
            for parsed in parse_fs_event(&event, &agents_dir, &pending_dir, &mut responders) {
                let _ = events_tx.send(parsed);
            }
        }

        // Execute effects (non-blocking)
        while let Ok(effect) = effects_rx.try_recv() {
            execute_effect(effect, &mut responders, &agents_dir)?;
        }

        thread::sleep(Duration::from_millis(10));
    }
}

fn execute_effect(
    effect: Effect,
    responders: &mut ResponderMap,
    agents_dir: &Path,
) -> io::Result<()> {
    match effect {
        Effect::SendResponse { responder, content } => {
            responders.send(responder, &content)?;
        }
        Effect::DispatchTask { agent_id, envelope, .. } => {
            let task_path = agents_dir.join(&agent_id).join("task.json");
            fs::write(&task_path, &envelope)?;
        }
        Effect::DeregisterAgent { agent_id } => {
            let _ = fs::remove_dir_all(agents_dir.join(&agent_id));
        }
        Effect::Log { level, message } => {
            match level {
                LogLevel::Debug => debug!("{message}"),
                LogLevel::Info => info!("{message}"),
                LogLevel::Warn => warn!("{message}"),
            }
        }
        Effect::ShutdownComplete => {}
    }
    Ok(())
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
fn test_submission_queues_task() {
    let state = PoolState::new(test_config());
    let (state, effects) = step(state, Event::Submission {
        content: "test".into(),
        responder: ResponderId(1),
    });

    assert_eq!(state.pending_tasks.len(), 1);
    assert!(effects.is_empty()); // No agent to dispatch to
}

#[test]
fn test_submission_dispatches_to_idle_agent() {
    let mut state = PoolState::new(test_config());
    state.agents.insert("agent-1".into(), AgentState::new());

    let (state, effects) = step(state, Event::Submission {
        content: "test".into(),
        responder: ResponderId(1),
    });

    assert!(state.pending_tasks.is_empty());
    assert!(matches!(
        state.agents.get("agent-1").unwrap().status,
        AgentStatus::Busy { .. }
    ));
    assert!(effects.iter().any(|e| matches!(e, Effect::DispatchTask { .. })));
}

#[test]
fn test_agent_response_sends_to_submitter() {
    let mut state = PoolState::new(test_config());
    state.agents.insert("agent-1".into(), AgentState {
        status: AgentStatus::Busy {
            task_id: TaskId(1),
            kind: TaskKind::Submission { responder: ResponderId(42) },
        },
        idle_ticks: 0,
    });

    let (state, effects) = step(state, Event::AgentResponse {
        agent_id: "agent-1".into(),
        response: "result".into(),
    });

    assert!(matches!(state.agents.get("agent-1").unwrap().status, AgentStatus::Idle));
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::SendResponse { responder: ResponderId(42), .. }
    )));
}

#[test]
fn test_health_check_timeout_deregisters() {
    let mut state = PoolState::new(PoolConfig {
        health_check_timeout_ticks: 3,
        ..test_config()
    });
    state.agents.insert("agent-1".into(), AgentState {
        status: AgentStatus::Busy {
            task_id: TaskId(1),
            kind: TaskKind::HealthCheck,
        },
        idle_ticks: 3, // At timeout threshold
    });

    let (state, effects) = step(state, Event::TimerTick);

    assert!(!state.agents.contains_key("agent-1"));
    assert!(effects.iter().any(|e| matches!(
        e,
        Effect::DeregisterAgent { agent_id } if agent_id == "agent-1"
    )));
}
```

These tests run instantly with no I/O, no threads, no timing issues.

---

## State Ownership Summary

| Data | Layer | Reason |
|------|-------|--------|
| `pending_tasks: VecDeque<PendingTask>` | 1 | Core queue logic |
| `agents: HashMap<String, AgentState>` | 1 | Core dispatch logic |
| `config: PoolConfig` | 1 | Affects state transitions |
| `next_id: u64` | 1 | ID generation is deterministic |
| `ResponderMap` | 3 | Maps IDs to I/O handles |
| `Listener` | 3 | Socket I/O |
| `Watcher` | 3 | FS I/O |
| `agents_dir: PathBuf` | 3 | File paths are I/O |
| `pending_dir: PathBuf` | 3 | File paths are I/O |

The key insight: **Layer 1 only knows about IDs (ResponderId, TaskId). Layer 3 maps IDs to actual I/O handles.**
