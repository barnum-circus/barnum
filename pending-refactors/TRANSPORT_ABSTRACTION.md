# Transport Abstraction Plan

## Dependencies

**Phase 1 can start now** - sync traits with `Box<dyn Trait>`

**Phase 2 (async) requires**: DAEMON_REFACTOR.md
- Converting traits from sync to async
- Requires tokio runtime to be established first

## Problem

The socket vs file distinction creates duplicate internal code paths:

```rust
// Client side - two separate functions with duplicate logic
pub fn submit(root: &Path, input: &str) -> io::Result<Response>;      // socket
pub fn submit_file(root: &Path, input: &str) -> io::Result<Response>; // file

// Daemon - two code paths for receiving submissions
// - Socket: accept(), read from stream
// - File: watch pending/, read task.json

// Agent communication - only file-based
// - Daemon writes agents/<name>/task.json
// - Agent polls for task.json, writes response.json
```

**Issues:**
1. Two internal code paths to maintain
2. Daemon has separate handling for socket vs file submissions
3. Agents can't use sockets even when not sandboxed

## Goals

1. **Internal simplicity:** Unify code paths behind a trait. Both transports go through the same internal logic.

2. **Identical caller API:** Both transports have the same interface - call a method, get a response back. The only difference is setup:

```rust
// Socket-based
let client = SocketClient::connect(&pool)?;
let response = client.submit(&task).await?;  // Response returned directly

// File-based
let client = FileClient::new(&pool)?;
let response = client.submit(&task).await?;  // Response returned directly (polling is internal)
```

The file-based implementation handles writing task.json and polling for response.json internally. Callers don't deal with files - they just call `submit()` and get a `Response` back.

```rust
// Client submits task - transport chosen at construction
let client = PoolClient::socket(pool_path)?;  // or PoolClient::file(pool_path)?
let response = client.submit(task_json).await?;

// Agent receives tasks - transport chosen at construction
let agent = AgentConnection::socket(pool_path, name)?;  // or AgentConnection::file(...)
loop {
    let task = agent.recv().await?;
    let response = process(task);
    agent.send(response).await?;
}
```

## Communication Patterns

There are two communication channels:

### 1. Submitter → Daemon (task submission)

```
Submitter                 Daemon
   |                         |
   |---- submit task ------->|
   |                         |
   |<--- response -----------|
   |                         |
```

**Socket transport:**
- Submitter connects to `daemon.sock`
- Submitter writes task, reads response
- Connection closed after each request

**File transport:**
- Submitter creates `pending/<uuid>/` directory
- Submitter writes `pending/<uuid>/task.json`
- Daemon reads task.json, later writes response.json
- Submitter polls for response.json, reads it, cleans up its own directory

### 2. Daemon → Agent (task dispatch)

```
Daemon                    Agent
   |                         |
   |---- dispatch task ----->|
   |                         |
   |<--- response -----------|
   |                         |
```

**Current (file-only):**
- Daemon writes `agents/<name>/task.json`
- Agent polls for task.json
- Agent writes `response.json`
- Daemon watches for response.json

**Potential socket transport:**
- Agent connects to daemon, keeps connection open
- Daemon writes task to agent's connection
- Agent writes response back
- Connection stays open for next task

## Proposed Traits

### For Submitters (sending tasks to the pool)

A **submitter** is code that wants to run a task on an agent. Examples: `gsd` CLI, test code, other programs.

```rust
/// A connection to the pool for submitting tasks.
///
/// Both implementations have identical usage:
///   let response = submitter.submit(&task).await?;
///
/// The transport details (socket connection vs file I/O) are handled internally.
#[async_trait]
pub trait PoolSubmitter: Send + Sync {
    /// Submit a task and wait for the response.
    /// Returns the Response directly - caller never deals with files or sockets.
    async fn submit(&self, task: &str) -> io::Result<Response>;
}

/// Socket-based submitter (fast, requires socket access)
pub struct SocketSubmitter {
    socket_path: PathBuf,
}

/// File-based submitter (works in sandboxes)
/// Internally: writes task.json, polls for response.json, cleans up its own directory
pub struct FileSubmitter {
    pending_dir: PathBuf,
}

impl PoolSubmitter for SocketSubmitter { ... }
impl PoolSubmitter for FileSubmitter { ... }
```

### For Agents (receiving tasks via get_task CLI)

An **agent** is a worker that processes tasks. The `get_task` CLI is how agents receive tasks.

Note: This is separate from `AgentChannel` (daemon-side). Here we're talking about what the agent uses.

```rust
/// Agent-side connection to the pool for receiving and completing tasks.
/// Used by the `get_task` CLI internally.
///
/// Both implementations have identical usage:
///   let task = agent_conn.recv().await?;
///   // ... agent does work (outside this code) ...
///   agent_conn.send(&response).await?;
///
/// The transport details are handled internally.
#[async_trait]
pub trait AgentReceiver: Send {
    /// Wait for the next task. Blocks until a task is available.
    async fn recv(&mut self) -> io::Result<TaskPayload>;

    /// Send a response for the current task.
    async fn send(&mut self, response: &str) -> io::Result<()>;
}

/// File-based agent receiver (current implementation)
/// Internally: polls for task.json, writes response.json
/// Daemon cleans up the files after reading the response.
pub struct FileAgentReceiver {
    agent_dir: PathBuf,
}

/// Socket-based agent receiver (keeps persistent connection to daemon)
/// Internally: reads task from stream, writes response to stream
pub struct SocketAgentReceiver {
    stream: UnixStream,
}

impl AgentReceiver for FileAgentReceiver { ... }
impl AgentReceiver for SocketAgentReceiver { ... }
```

## Socket-Based Agent Protocol

For agents to use sockets, we need a protocol for the persistent connection:

```
Agent                                Daemon
   |                                    |
   |---- REGISTER {name} -------------->|  Agent connects and registers
   |                                    |
   |<--- TASK {id, content} ------------|  Daemon sends task
   |                                    |
   |---- RESPONSE {id, content} ------->|  Agent sends response
   |                                    |
   |<--- TASK {id, content} ------------|  Next task...
   |                                    |
   |---- HEARTBEAT -------------------->|  (optional) Keep-alive
   |                                    |
   |---- DEREGISTER ------------------->|  Agent disconnects
   |                                    |
```

**Message format (JSON lines):**
```json
{"type": "register", "name": "agent-1"}
{"type": "task", "id": "abc123", "content": {...}}
{"type": "response", "id": "abc123", "content": "..."}
{"type": "heartbeat"}
{"type": "deregister"}
```

## Daemon Changes

The daemon uses a trait for agent communication, not an enum match:

```rust
/// Trait for daemon's communication with an agent.
/// The daemon calls these methods without knowing the transport.
#[async_trait]
trait AgentChannel: Send {
    /// Send a task to the agent.
    async fn dispatch(&mut self, task: &Task) -> io::Result<()>;

    /// Check if a response is available (non-blocking).
    async fn poll_response(&mut self) -> io::Result<Option<String>>;

    /// Send a heartbeat check (if applicable).
    async fn heartbeat(&mut self) -> io::Result<()>;
}

struct FileAgentChannel {
    agent_dir: PathBuf,
}

struct SocketAgentChannel {
    stream: UnixStream,
}

impl AgentChannel for FileAgentChannel {
    async fn dispatch(&mut self, task: &Task) -> io::Result<()> {
        fs::write(self.agent_dir.join("task.json"), &task.content)?;
        Ok(())
    }
    // ...
}

impl AgentChannel for SocketAgentChannel {
    async fn dispatch(&mut self, task: &Task) -> io::Result<()> {
        let msg = json!({"type": "task", "content": task.content});
        self.stream.write_all(msg.to_string().as_bytes()).await?;
        self.stream.write_all(b"\n").await?;
        Ok(())
    }
    // ...
}

/// Agent state holds a boxed trait object - no matching on transport type.
struct AgentState {
    channel: Box<dyn AgentChannel>,
    in_flight: Option<InFlightTask>,
}

impl AgentState {
    async fn dispatch(&mut self, task: Task) -> io::Result<()> {
        // No match - just call the trait method
        self.channel.dispatch(&task).await?;
        self.in_flight = Some(InFlightTask { /* ... */ });
        Ok(())
    }
}
```

The daemon code never matches on transport type. All transport-specific logic is encapsulated in the trait implementations.

## CLI Changes

The CLI explicitly chooses transport:

```rust
// submit_task command (for submitters)
let submitter: Box<dyn PoolSubmitter> = if use_file {
    Box::new(FileSubmitter::new(&pool_path)?)
} else {
    Box::new(SocketSubmitter::connect(&pool_path)?)
};
let response = submitter.submit(&task_json).await?;

// get_task command (used by agents to receive tasks)
let receiver: Box<dyn AgentReceiver> = if use_file {
    Box::new(FileAgentReceiver::new(&pool_path, &name)?)
} else {
    Box::new(SocketAgentReceiver::connect(&pool_path, &name)?)
};
loop {
    let task = receiver.recv().await?;
    println!("{}", serde_json::to_string(&task)?);
    // ... agent does work (externally, not in this code) ...
    receiver.send(&response).await?;
}
```

The `--file` flag (or similar) determines which transport to use.

## Migration Path

### Phase 1: Extract traits (no behavior change)
1. Define `PoolClient` trait
2. Implement `SocketClient` (wraps current `submit()`)
3. Implement `FileClient` (wraps current `submit_file()`)
4. Update CLI to use trait with explicit transport choice

### Phase 2: Unify daemon submission handling
1. Both socket and file submissions produce the same internal `Task` struct
2. Single code path for enqueueing tasks
3. `ResponseTarget` enum already handles response routing

### Phase 3: Add socket-based agents (optional)
1. Define `AgentConnection` trait
2. Implement `FileAgent` (current behavior)
3. Implement `SocketAgent` (new)
4. Update daemon to accept agent socket connections
5. Update CLI `get_task` to use trait

## Open Questions

1. **Socket agent registration:** Should socket agents register on the same `daemon.sock`, or a separate `agents.sock`?

2. **Mixed mode:** Can the daemon handle some agents on sockets and others on files simultaneously? (Yes, with `AgentChannel` enum)

3. **Backward compatibility:** Keep `submit()` and `submit_file()` as convenience functions that use the traits internally?

4. **Async vs sync:** Should the traits be async? The file-based implementation has polling which benefits from async. Socket is naturally async.

## Future Enhancements (not in scope)

- **Auto-detection:** Factory function that tries socket, falls back to file
- **Remote pools:** TCP transport for pools on other machines

## Benefits

1. **Internal simplicity:** One code path in the daemon, not two
2. **Less duplication:** Shared logic for task handling regardless of transport
3. **Testability:** Can mock the trait for unit tests
4. **Flexibility:** Easy to add new transports later (e.g., TCP for remote pools)
5. **Performance:** Socket-based agents would be faster than file polling

---

## Implementation Tasks

### Phase 1: Sync Traits (before daemon refactor)

Use sync traits with `Box<dyn Trait>`. This provides the abstraction benefits without requiring async runtime.

| Status | Task | Description |
|--------|------|-------------|
| [ ] | 1.1 | Define sync `PoolSubmitter` trait in new `transport.rs` module |
| [ ] | 1.2 | Implement `SocketSubmitter` (wraps current `submit()` logic) |
| [ ] | 1.3 | Implement `FileSubmitter` (wraps current `submit_file()` logic) |
| [ ] | 1.4 | Update public API: `submit()` and `submit_file()` become thin wrappers |
| [ ] | 1.5 | Define sync `AgentChannel` trait for daemon→agent communication |
| [ ] | 1.6 | Implement `FileAgentChannel` (current behavior) |
| [ ] | 1.7 | Update `AgentState` to use `Box<dyn AgentChannel>` |
| [ ] | 1.8 | Update tests |

### Phase 2: Async Traits (after daemon refactor)

After the daemon uses tokio, convert traits to async:

| Status | Task | Description |
|--------|------|-------------|
| [ ] | 2.1 | Add `async_trait` dependency |
| [ ] | 2.2 | Convert `PoolSubmitter` to async trait |
| [ ] | 2.3 | Convert `AgentChannel` to async trait |
| [ ] | 2.4 | Update implementations to use async I/O |

### Phase 3: Socket-based Agents (optional, after Phase 2)

| Status | Task | Description |
|--------|------|-------------|
| [ ] | 3.1 | Define socket agent protocol (JSON lines) |
| [ ] | 3.2 | Implement `SocketAgentChannel` (daemon-side) |
| [ ] | 3.3 | Update daemon to accept agent socket connections |
| [ ] | 3.4 | Implement `SocketAgentReceiver` (for `get_task` CLI) |
| [ ] | 3.5 | Add `--socket` flag to `get_task` CLI |

---

## Task 1.1: Define sync `PoolSubmitter` trait

**File:** `crates/agent_pool/src/transport.rs` (new)

```rust
use crate::Response;
use std::io;
use std::path::Path;

/// Trait for submitting tasks to the pool.
///
/// Both implementations have identical usage:
///   let response = submitter.submit(&task)?;
///
/// The transport details (socket connection vs file I/O) are handled internally.
pub trait PoolSubmitter: Send + Sync {
    /// Submit a task and wait for the response.
    fn submit(&self, task: &str) -> io::Result<Response>;
}
```

## Task 1.2: Implement `SocketSubmitter`

```rust
pub struct SocketSubmitter {
    socket_path: PathBuf,
}

impl SocketSubmitter {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let socket_path = root.as_ref().join(SOCKET_NAME);
        Self { socket_path }
    }
}

impl PoolSubmitter for SocketSubmitter {
    fn submit(&self, task: &str) -> io::Result<Response> {
        // Current submit() logic moves here
        // Connect to socket, write task, read response
    }
}
```

## Task 1.3: Implement `FileSubmitter`

```rust
pub struct FileSubmitter {
    pending_dir: PathBuf,
}

impl FileSubmitter {
    pub fn new(root: impl AsRef<Path>) -> Self {
        let pending_dir = root.as_ref().join(PENDING_DIR);
        Self { pending_dir }
    }
}

impl PoolSubmitter for FileSubmitter {
    fn submit(&self, task: &str) -> io::Result<Response> {
        // Current submit_file() logic moves here:
        // 1. Create pending/<uuid>/ directory (submitter owns this)
        // 2. Write pending/<uuid>/task.json
        // 3. Poll for pending/<uuid>/response.json
        // 4. Read response, clean up directory (submitter's responsibility)
    }
}
```

## Task 1.5: Define sync `AgentChannel` trait

This is **daemon-side** - how the daemon communicates with agents. Not used by agents themselves.

```rust
/// Trait for daemon's communication with an agent.
/// The daemon calls these methods without knowing the transport.
///
/// Cleanup is the daemon's responsibility since it owns the agent communication.
pub trait AgentChannel: Send {
    /// Send a task to the agent (daemon → agent).
    fn dispatch(&mut self, envelope: &str) -> io::Result<()>;

    /// Check if a response is available (non-blocking).
    /// Returns the response content if available.
    fn poll_response(&mut self) -> io::Result<Option<String>>;

    /// Clean up after task completion (daemon cleans up both task and response files).
    fn cleanup(&mut self) -> io::Result<()>;
}
```

## Task 1.6: Implement `FileAgentChannel`

```rust
pub struct FileAgentChannel {
    agent_dir: PathBuf,
}

impl FileAgentChannel {
    pub fn new(agent_dir: PathBuf) -> Self {
        Self { agent_dir }
    }
}

impl AgentChannel for FileAgentChannel {
    fn dispatch(&mut self, envelope: &str) -> io::Result<()> {
        fs::write(self.agent_dir.join(TASK_FILE), envelope)
    }

    fn poll_response(&mut self) -> io::Result<Option<String>> {
        let response_path = self.agent_dir.join(RESPONSE_FILE);
        match fs::read_to_string(&response_path) {
            Ok(content) => Ok(Some(content)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn cleanup(&mut self) -> io::Result<()> {
        let _ = fs::remove_file(self.agent_dir.join(TASK_FILE));
        let _ = fs::remove_file(self.agent_dir.join(RESPONSE_FILE));
        Ok(())
    }
}
```

## Task 1.7: Update `AgentState` to use trait

```rust
struct AgentState {
    status: AgentStatus,
    last_activity: Instant,
    channel: Box<dyn AgentChannel>,
}

impl AgentState {
    fn new(agent_dir: PathBuf) -> Self {
        Self {
            status: AgentStatus::Idle,
            last_activity: Instant::now(),
            channel: Box::new(FileAgentChannel::new(agent_dir)),
        }
    }
}
```

In `dispatch_to()`:
```rust
// Before:
fs::write(&task_path, envelope.to_string())?;

// After:
agent.channel.dispatch(&envelope.to_string())?;
```

In `complete_task()`:
```rust
// Before:
let output = fs::read_to_string(response_path)?;
let _ = fs::remove_file(agent_dir.join(TASK_FILE));
let _ = fs::remove_file(response_path);

// After:
let output = agent.channel.poll_response()?.ok_or_else(|| ...)?;
agent.channel.cleanup()?;
```
