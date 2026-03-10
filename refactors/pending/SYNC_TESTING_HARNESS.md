# Synchronous Testing Harness

**Status:** Future work. This is a design for in-memory testing to eliminate filesystem timing issues. Not implemented.

**Note:** Current tests pass with `--test-threads=1`. This design would allow parallel tests by eliminating real I/O entirely.

## Problem

Current tests use real filesystem operations and timing:

```rust
// Current approach - relies on real I/O and timing
let handle = spawn(&pool_path)?;
fs::create_dir_all(agent_dir)?;  // Register agent
fs::write(response_path, "done")?;  // Simulate agent response
thread::sleep(Duration::from_millis(50));  // Hope timing works out
```

**Issues:**
1. Tests are flaky due to filesystem timing (FSEvents latency, etc.)
2. Can't test protocol edge cases without real timing
3. Hard to simulate "race conditions" deterministically
4. Slow - each test needs real I/O and sleeps

## Goal

A synchronous, in-memory testing harness where:
- Agents respond immediately (no real I/O)
- Time is controlled, not real
- Protocol interactions are deterministic
- Edge cases and "race conditions" can be tested reliably

## Design

### Core Abstraction: `Transport` trait

Instead of the daemon using real filesystem and sockets, abstract the transport:

```rust
/// How the daemon communicates with agents.
trait Transport {
    /// Send a task to an agent.
    fn dispatch(&mut self, agent_id: &str, task: &str) -> io::Result<()>;

    /// Poll for a response from an agent (non-blocking).
    fn poll_response(&mut self, agent_id: &str) -> io::Result<Option<String>>;

    /// Check which agents are currently registered.
    fn registered_agents(&self) -> Vec<String>;

    /// Deregister an agent.
    fn deregister(&mut self, agent_id: &str) -> io::Result<()>;
}
```

### Real Transport (Production)

The production transport uses files:

```rust
struct FileTransport {
    agents_dir: PathBuf,
}

impl Transport for FileTransport {
    fn dispatch(&mut self, agent_id: &str, task: &str) -> io::Result<()> {
        let task_path = self.agents_dir.join(agent_id).join("task.json");
        fs::write(&task_path, task)
    }

    fn poll_response(&mut self, agent_id: &str) -> io::Result<Option<String>> {
        let response_path = self.agents_dir.join(agent_id).join("response.json");
        match fs::read_to_string(&response_path) {
            Ok(content) => {
                fs::remove_file(&response_path)?;
                Ok(Some(content))
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e),
        }
    }

    fn registered_agents(&self) -> Vec<String> {
        fs::read_dir(&self.agents_dir)
            .into_iter()
            .flatten()
            .filter_map(|e| e.ok())
            .filter(|e| e.path().is_dir())
            .filter_map(|e| e.file_name().into_string().ok())
            .collect()
    }

    fn deregister(&mut self, agent_id: &str) -> io::Result<()> {
        let agent_dir = self.agents_dir.join(agent_id);
        fs::remove_dir_all(&agent_dir)
    }
}
```

### Sync Transport (Testing)

The test transport is fully synchronous and in-memory:

```rust
/// A mock agent that responds synchronously.
trait MockAgent: Send {
    /// Called when a task is dispatched. Returns the response immediately.
    fn handle(&mut self, task: &str) -> String;
}

/// Synchronous, in-memory transport for testing.
struct SyncTransport {
    agents: HashMap<String, Box<dyn MockAgent>>,
    pending_responses: HashMap<String, String>,
}

impl SyncTransport {
    fn new() -> Self {
        Self {
            agents: HashMap::new(),
            pending_responses: HashMap::new(),
        }
    }

    /// Register a mock agent.
    fn register(&mut self, id: &str, agent: impl MockAgent + 'static) {
        self.agents.insert(id.to_string(), Box::new(agent));
    }
}

impl Transport for SyncTransport {
    fn dispatch(&mut self, agent_id: &str, task: &str) -> io::Result<()> {
        let agent = self.agents.get_mut(agent_id)
            .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "agent not found"))?;

        // Agent responds immediately
        let response = agent.handle(task);
        self.pending_responses.insert(agent_id.to_string(), response);
        Ok(())
    }

    fn poll_response(&mut self, agent_id: &str) -> io::Result<Option<String>> {
        Ok(self.pending_responses.remove(agent_id))
    }

    fn registered_agents(&self) -> Vec<String> {
        self.agents.keys().cloned().collect()
    }

    fn deregister(&mut self, agent_id: &str) -> io::Result<()> {
        self.agents.remove(agent_id);
        self.pending_responses.remove(agent_id);
        Ok(())
    }
}
```

### Example Mock Agents

```rust
/// Agent that echoes the task back.
struct EchoAgent;

impl MockAgent for EchoAgent {
    fn handle(&mut self, task: &str) -> String {
        task.to_string()
    }
}

/// Agent that responds with a fixed value.
struct FixedAgent(String);

impl MockAgent for FixedAgent {
    fn handle(&mut self, _task: &str) -> String {
        self.0.clone()
    }
}

/// Agent that tracks how many tasks it received.
struct CountingAgent {
    count: usize,
}

impl MockAgent for CountingAgent {
    fn handle(&mut self, _task: &str) -> String {
        self.count += 1;
        format!(r#"{{"count": {}}}"#, self.count)
    }
}

/// Agent that fails on first N tasks, then succeeds.
struct FailThenSucceedAgent {
    failures_remaining: usize,
}

impl MockAgent for FailThenSucceedAgent {
    fn handle(&mut self, task: &str) -> String {
        if self.failures_remaining > 0 {
            self.failures_remaining -= 1;
            "invalid json".to_string()  // Causes retry
        } else {
            task.to_string()
        }
    }
}
```

### Synchronous Pool for Testing

```rust
/// A pool that runs synchronously for testing.
struct SyncPool {
    transport: SyncTransport,
    pending: VecDeque<Task>,
    agents: HashMap<String, AgentState>,
}

impl SyncPool {
    fn new() -> Self {
        Self {
            transport: SyncTransport::new(),
            pending: VecDeque::new(),
            agents: HashMap::new(),
        }
    }

    /// Register a mock agent.
    fn register(&mut self, id: &str, agent: impl MockAgent + 'static) {
        self.transport.register(id, agent);
        self.agents.insert(id.to_string(), AgentState::new());
    }

    /// Submit a task and run until it completes.
    fn submit_sync(&mut self, task: &str) -> Response {
        self.pending.push_back(Task { content: task.to_string(), .. });
        self.run_until_complete()
    }

    /// Run the pool until all tasks are done.
    fn run_until_complete(&mut self) -> Response {
        // Dispatch pending to available agents
        while let Some(agent_id) = self.find_available() {
            if let Some(task) = self.pending.pop_front() {
                self.transport.dispatch(&agent_id, &task.content).unwrap();
                self.agents.get_mut(&agent_id).unwrap().in_flight = Some(task);
            } else {
                break;
            }
        }

        // Collect responses (synchronous - always ready)
        for (agent_id, agent) in &mut self.agents {
            if agent.in_flight.is_some() {
                if let Some(response) = self.transport.poll_response(agent_id).unwrap() {
                    let task = agent.in_flight.take().unwrap();
                    return Response::processed(response);
                }
            }
        }

        panic!("no response received");
    }
}
```

## Test Examples

### Basic dispatch

```rust
#[test]
fn single_agent_single_task() {
    let mut pool = SyncPool::new();
    pool.register("agent-1", EchoAgent);

    let response = pool.submit_sync(r#"{"msg": "hello"}"#);

    assert!(matches!(response, Response::Processed { stdout, .. } if stdout.contains("hello")));
}
```

### Multiple agents

```rust
#[test]
fn tasks_distributed_across_agents() {
    let mut pool = SyncPool::new();
    pool.register("agent-1", CountingAgent { count: 0 });
    pool.register("agent-2", CountingAgent { count: 0 });

    // Submit multiple tasks
    pool.submit_sync("task-1");
    pool.submit_sync("task-2");

    // Both agents should have been used
    // (deterministic - no timing involved)
}
```

### Protocol edge cases

```rust
#[test]
fn retry_on_invalid_response() {
    let mut pool = SyncPool::new();
    pool.register("agent-1", FailThenSucceedAgent { failures_remaining: 2 });

    // First two attempts fail, third succeeds
    let response = pool.submit_sync("task");

    assert!(matches!(response, Response::Processed { .. }));
}
```

### Keepalive behavior

```rust
#[test]
fn initial_keepalive_before_real_tasks() {
    let mut pool = SyncPool::with_config(DaemonConfig {
        initial_keepalive: true,
        ..Default::default()
    });

    // Agent that tracks task order
    let mut tasks_received = Vec::new();
    pool.register("agent-1", |task: &str| {
        tasks_received.push(task.to_string());
        if task.contains("Keepalive") {
            r#"{"id": "ping-123"}"#.to_string()
        } else {
            task.to_string()
        }
    });

    pool.submit_sync("real-task");

    // First task should be keepalive, second should be real
    assert!(tasks_received[0].contains("Keepalive"));
    assert_eq!(tasks_received[1], "real-task");
}
```

## Implementation Tasks

### Task 1: Extract Transport trait

**Files:** `crates/troupe/src/transport.rs`

Define the `Transport` trait and move file operations behind it.

### Task 2: Implement FileTransport

**Files:** `crates/troupe/src/transport.rs`

Production transport using filesystem.

### Task 3: Implement SyncTransport

**Files:** `crates/troupe/src/transport.rs`

In-memory transport for testing.

### Task 4: Refactor PoolState to use Transport

**Files:** `crates/troupe/src/daemon.rs`

Make `PoolState` generic over `Transport` instead of hardcoding file operations.

### Task 5: Create SyncPool test helper

**Files:** `crates/troupe/src/testing.rs`

A synchronous pool wrapper for tests.

### Task 6: Migrate existing tests

**Files:** `crates/troupe/tests/*.rs`

Convert existing tests to use `SyncPool` where appropriate. Keep some integration tests that exercise real I/O.

## Benefits

1. **Deterministic tests** - No flakiness from timing or filesystem latency
2. **Fast tests** - No I/O, no sleeps, instant feedback
3. **Edge case coverage** - Can test retry logic, keepalive behavior, etc.
4. **Protocol validation** - Agents that follow protocol exactly, exposing bugs
5. **Simpler test setup** - No temp directories, no cleanup

## Relationship to Other Refactors

This refactor is largely independent but complements:

- **TRANSPORT_ABSTRACTION.md** - Both involve abstracting transport, but that doc focuses on socket vs file for production. This focuses on real vs mock for testing.
- **DAEMON_REFACTOR.md** - The tokio refactor is orthogonal. SyncTransport tests the protocol logic; async is about I/O multiplexing.
- **KEEPALIVE_PLAN.md** - Keepalive tests will be much easier with SyncPool.
