# Notify-Based Agent Waiting

## Goal

Replace sleep-based polling with `notify`-based file watching for agents waiting for tasks. Test agents should use the same code path as real agents (via CLI or library).

## Current Architecture

### The Agent Protocol (File-Based)

Agents communicate with the daemon through files in `<pool>/agents/<agent_name>/`:
- `task.json` - Written by daemon when assigning work
- `response.json` - Written by agent when work is complete

**State Machine:**

| task.json | response.json | Meaning |
|-----------|---------------|---------|
| absent | absent | Idle - waiting for task |
| present | absent | Task pending - agent should process |
| present | present | Agent done - daemon should cleanup |
| absent | present | Cleanup in progress - transitionary, do nothing |

The daemon deletes task.json first, then response.json. This means `(absent, present)` is a valid transitionary state that agents will briefly observe during cleanup. Agents should simply wait - they only act when `task.exists() && !response.exists()`.

### Current Implementation: CLI

`crates/agent_pool_cli/src/main.rs`:

```rust
// Lines 195-239
fn wait_for_task(
    task_file: &std::path::Path,
    response_file: &std::path::Path,
    name: &str,
) -> Result<String, String> {
    loop {
        if task_file.exists() && !response_file.exists() {
            // Read and return task
            return Ok(...);
        }
        thread::sleep(Duration::from_millis(100));  // POLLING!
    }
}
```

```rust
// Lines 532-535 (NextTask command)
// Wait for daemon to consume the response
while task_file.exists() {
    thread::sleep(Duration::from_millis(100));  // POLLING!
}
```

### Current Implementation: Test Agents

`crates/agent_pool/tests/common/mod.rs`:

```rust
// Lines 164-207
while running_clone.load(Ordering::SeqCst) {
    if task_file.exists() && !response_file.exists() {
        // Process task...
    }
    thread::sleep(Duration::from_millis(10));  // POLLING!
}
```

`crates/gsd_config/tests/common/mod.rs` has identical polling logic.

### Problems

1. **Polling is wasteful** - Constant CPU wake-ups, wastes energy, adds latency
2. **Code duplication** - Three separate implementations of the same logic
3. **Race conditions** - Polling windows can cause agents to see stale state, leading to duplicate processing
4. **Wrong primitive** - Sleep-based polling is not the right abstraction for "wait for file to exist"

## Target Architecture

### Core Primitive: `AgentTransport`

A struct in the `agent_pool` library that encapsulates the agent side of the file protocol:

```rust
// crates/agent_pool/src/agent/transport.rs

use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use std::sync::mpsc;

/// Agent-side transport for communicating with the daemon via files.
///
/// Uses `notify` for event-driven file watching instead of polling.
pub struct AgentTransport {
    agent_dir: PathBuf,
    task_file: PathBuf,
    response_file: PathBuf,

    // File watcher and event channel
    _watcher: RecommendedWatcher,
    events_rx: mpsc::Receiver<notify::Result<notify::Event>>,
}

/// A task received from the daemon.
pub struct Task {
    pub kind: TaskKind,
    pub content: serde_json::Value,
}

pub enum TaskKind {
    /// Regular task with work to do
    Work,
    /// Heartbeat - respond with any valid JSON
    Heartbeat,
    /// Agent was kicked - should exit
    Kicked { reason: String },
}

impl AgentTransport {
    /// Create a new transport, registering with the daemon.
    ///
    /// Creates the agent directory if it doesn't exist.
    pub fn new(pool_root: &Path, agent_name: &str) -> io::Result<Self>;

    /// Block until a task is available.
    ///
    /// Returns when: task.json exists AND response.json does not exist.
    /// This is event-driven via `notify`, not polling.
    pub fn wait_for_task(&self) -> io::Result<Task>;

    /// Write a response and wait for the daemon to clean up.
    ///
    /// This method:
    /// 1. Writes response.json
    /// 2. Waits for task.json to be deleted (daemon acknowledged)
    /// 3. Returns (caller should then call wait_for_task for next task)
    pub fn write_response(&self, response: &str) -> io::Result<()>;
}
```

### Why This Is The Right Primitive

1. **Encapsulates complexity** - File watching, event handling, state checking all in one place
2. **Event-driven** - No polling, immediate response to file changes
3. **Reusable** - CLI and tests use the same implementation
4. **Testable** - Can unit test the transport in isolation
5. **Clear state machine** - Methods correspond to valid state transitions

### Making Impossible States Unrepresentable

The file system is shared state we can't fully control, but we can:

1. **API enforces valid transitions:**
   ```rust
   impl AgentTransport {
       // Can only wait for task (not write response) from initial state
       pub fn wait_for_task(&self) -> io::Result<Task>;

       // After getting a task, you MUST write a response
       // Could use typestate to enforce this at compile time
   }
   ```

2. **Handle transitionary states gracefully:**
   ```rust
   // (absent, present) is a valid transitionary state during cleanup
   // Just wait - don't act, don't error
   if response_file.exists() && !task_file.exists() {
       return Ok(None);  // Keep waiting
   }
   ```

3. **Consider typestate pattern** (optional, may be overkill):
   ```rust
   pub struct AgentTransport<S: AgentState> {
       // ...
       _state: PhantomData<S>,
   }

   pub struct Idle;
   pub struct HasTask;

   impl AgentTransport<Idle> {
       pub fn wait_for_task(self) -> io::Result<(Task, AgentTransport<HasTask>)>;
   }

   impl AgentTransport<HasTask> {
       pub fn write_response(self, response: &str) -> io::Result<AgentTransport<Idle>>;
   }
   ```

### File Watching Implementation

```rust
impl AgentTransport {
    pub fn wait_for_task(&self) -> io::Result<Task> {
        // Check initial state - might already have a task
        if let Some(task) = self.try_read_task()? {
            return Ok(task);
        }

        // Wait for file events
        loop {
            // Block on event channel (no polling!)
            let event = self.events_rx.recv()
                .map_err(|_| io::Error::new(io::ErrorKind::Other, "watcher disconnected"))?;

            // Handle event
            match event {
                Ok(event) => {
                    // Check if state changed to "task available"
                    if let Some(task) = self.try_read_task()? {
                        return Ok(task);
                    }
                }
                Err(e) => {
                    return Err(io::Error::new(io::ErrorKind::Other, e));
                }
            }
        }
    }

    fn try_read_task(&self) -> io::Result<Option<Task>> {
        if self.task_file.exists() && !self.response_file.exists() {
            let content = fs::read_to_string(&self.task_file)?;
            let task = parse_task_envelope(&content)?;
            Ok(Some(task))
        } else {
            Ok(None)
        }
    }
}
```

## Tasks

### Task 1: Create `AgentTransport` struct in `agent_pool` library

**Goal:** Add the core transport abstraction with notify-based waiting.

**File:** `crates/agent_pool/src/agent/mod.rs` (new file)

#### 1.1: Create the module structure

**File:** `crates/agent_pool/src/lib.rs`

Add:
```rust
mod agent;
pub use agent::{AgentTransport, Task, TaskKind};
```

**File:** `crates/agent_pool/src/agent/mod.rs` (new)

```rust
//! Agent-side transport for the file-based protocol.

mod transport;

pub use transport::{AgentTransport, Task, TaskKind};
```

#### 1.2: Implement `AgentTransport::new`

**File:** `crates/agent_pool/src/agent/transport.rs` (new)

```rust
use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc;

use crate::{AGENTS_DIR, RESPONSE_FILE, TASK_FILE};

pub struct AgentTransport {
    agent_dir: PathBuf,
    task_file: PathBuf,
    response_file: PathBuf,
    _watcher: RecommendedWatcher,
    events_rx: mpsc::Receiver<notify::Result<notify::Event>>,
}

impl AgentTransport {
    pub fn new(pool_root: &Path, agent_name: &str) -> io::Result<Self> {
        let agent_dir = pool_root.join(AGENTS_DIR).join(agent_name);
        fs::create_dir_all(&agent_dir)?;

        let task_file = agent_dir.join(TASK_FILE);
        let response_file = agent_dir.join(RESPONSE_FILE);

        // Set up file watcher
        let (events_tx, events_rx) = mpsc::channel();
        let mut watcher = RecommendedWatcher::new(
            move |res| { let _ = events_tx.send(res); },
            Config::default(),
        ).map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        watcher.watch(&agent_dir, RecursiveMode::NonRecursive)
            .map_err(|e| io::Error::new(io::ErrorKind::Other, e))?;

        Ok(Self {
            agent_dir,
            task_file,
            response_file,
            _watcher: watcher,
            events_rx,
        })
    }
}
```

#### 1.3: Implement `Task` and `TaskKind` types

**File:** `crates/agent_pool/src/agent/transport.rs`

```rust
/// A task received from the daemon.
#[derive(Debug)]
pub struct Task {
    pub kind: TaskKind,
    pub raw: String,
}

#[derive(Debug)]
pub enum TaskKind {
    /// Regular task - agent should process and respond
    Work { content: serde_json::Value },
    /// Heartbeat - respond with any valid JSON to confirm alive
    Heartbeat,
    /// Agent was kicked - should exit gracefully
    Kicked { reason: String },
}

fn parse_task(raw: &str) -> io::Result<Task> {
    let envelope: serde_json::Value = serde_json::from_str(raw)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

    let kind_str = envelope.get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("Task");

    let kind = match kind_str {
        "Kicked" => {
            let reason = envelope.get("reason")
                .and_then(|r| r.as_str())
                .unwrap_or("unknown")
                .to_string();
            TaskKind::Kicked { reason }
        }
        "Heartbeat" => TaskKind::Heartbeat,
        _ => {
            let content = envelope.get("task")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            TaskKind::Work { content }
        }
    };

    Ok(Task { kind, raw: raw.to_string() })
}
```

#### 1.4: Implement `wait_for_task`

**File:** `crates/agent_pool/src/agent/transport.rs`

```rust
impl AgentTransport {
    /// Block until a task is available.
    ///
    /// Returns when task.json exists AND response.json does not exist.
    pub fn wait_for_task(&self) -> io::Result<Task> {
        // Check initial state
        if let Some(task) = self.try_read_task()? {
            return Ok(task);
        }

        // Wait for file events
        loop {
            let event = self.events_rx.recv()
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "watcher channel closed"))?;

            if let Err(e) = event {
                return Err(io::Error::new(io::ErrorKind::Other, format!("watch error: {e}")));
            }

            // Check if we now have a task
            if let Some(task) = self.try_read_task()? {
                return Ok(task);
            }
        }
    }

    fn try_read_task(&self) -> io::Result<Option<Task>> {
        // Only return a task when: task exists AND response does not
        // Other states:
        // - (absent, absent): Idle, waiting for task
        // - (present, present): Agent done, daemon cleaning up
        // - (absent, present): Cleanup in progress (transitionary)
        if self.task_file.exists() && !self.response_file.exists() {
            let raw = fs::read_to_string(&self.task_file)?;
            let task = parse_task(&raw)?;
            Ok(Some(task))
        } else {
            Ok(None)
        }
    }
}
```

#### 1.5: Implement `write_response`

**File:** `crates/agent_pool/src/agent/transport.rs`

```rust
impl AgentTransport {
    /// Write response and wait for daemon to acknowledge (delete task file).
    pub fn write_response(&self, response: &str) -> io::Result<()> {
        fs::write(&self.response_file, response)?;

        // Wait for daemon to delete task file (acknowledgment)
        while self.task_file.exists() {
            let event = self.events_rx.recv()
                .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "watcher channel closed"))?;

            if let Err(e) = event {
                return Err(io::Error::new(io::ErrorKind::Other, format!("watch error: {e}")));
            }
        }

        Ok(())
    }
}
```

#### 1.6: Add accessor for agent directory path

**File:** `crates/agent_pool/src/agent/transport.rs`

```rust
impl AgentTransport {
    /// Get the agent directory path.
    pub fn agent_dir(&self) -> &Path {
        &self.agent_dir
    }

    /// Get the agent name.
    pub fn agent_name(&self) -> &str {
        self.agent_dir.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown")
    }
}
```

---

### Task 2: Update CLI to use `AgentTransport`

**Goal:** Replace polling in CLI with the library's notify-based implementation.

#### 2.1: Remove `wait_for_task` function from CLI

**File:** `crates/agent_pool_cli/src/main.rs`

Delete lines 195-239 (the `wait_for_task` function).

#### 2.2: Update `GetTask` / `Register` commands

**File:** `crates/agent_pool_cli/src/main.rs`

Before (lines 466-491):
```rust
Command::GetTask { pool, name } | Command::Register { pool, name } => {
    let root = resolve_pool(&pool);
    // ... validation ...
    let agent_dir = root.join(AGENTS_DIR).join(&name);
    if let Err(e) = fs::create_dir_all(&agent_dir) { ... }
    let task_file = agent_dir.join(TASK_FILE);
    let response_file = agent_dir.join(RESPONSE_FILE);
    match wait_for_task(&task_file, &response_file, &name) {
        Ok(output) => println!("{output}"),
        Err(e) => { ... }
    }
}
```

After:
```rust
Command::GetTask { pool, name } | Command::Register { pool, name } => {
    let root = resolve_pool(&pool);

    if !root.join(PENDING_DIR).exists() {
        eprintln!("Daemon not ready (pending directory doesn't exist)");
        return ExitCode::FAILURE;
    }

    let transport = match AgentTransport::new(&root, &name) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Failed to create agent transport: {e}");
            return ExitCode::FAILURE;
        }
    };

    match transport.wait_for_task() {
        Ok(task) => {
            let output = format_task_output(&task, &name, transport.agent_dir());
            println!("{output}");
        }
        Err(e) => {
            eprintln!("get_task failed: {e}");
            return ExitCode::FAILURE;
        }
    }
}
```

#### 2.3: Update `NextTask` command

**File:** `crates/agent_pool_cli/src/main.rs`

Before (lines 493-545):
```rust
Command::NextTask { pool, name, data } => {
    // ... setup ...
    if let Err(e) = fs::write(&response_file, &data) { ... }

    // Wait for daemon to consume the response (task file removed)
    while task_file.exists() {
        thread::sleep(Duration::from_millis(100));  // POLLING
    }

    // Wait for next task
    match wait_for_task(&task_file, &response_file, &name) { ... }
}
```

After:
```rust
Command::NextTask { pool, name, data } => {
    let root = resolve_pool(&pool);

    let transport = match AgentTransport::new(&root, &name) {
        Ok(t) => t,
        Err(e) => {
            eprintln!("Failed to create agent transport: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Write response and wait for daemon to acknowledge
    if let Err(e) = transport.write_response(&data) {
        eprintln!("Failed to write response: {e}");
        return ExitCode::FAILURE;
    }

    // Wait for next task
    match transport.wait_for_task() {
        Ok(task) => {
            let output = format_task_output(&task, &name, transport.agent_dir());
            println!("{output}");
        }
        Err(e) => {
            eprintln!("get_task failed: {e}");
            return ExitCode::FAILURE;
        }
    }
}
```

#### 2.4: Add helper function for task output formatting

**File:** `crates/agent_pool_cli/src/main.rs`

```rust
fn format_task_output(task: &Task, agent_name: &str, agent_dir: &Path) -> String {
    let response_file = agent_dir.join(RESPONSE_FILE);

    let (kind, content) = match &task.kind {
        TaskKind::Work { content } => ("Task", content.clone()),
        TaskKind::Heartbeat => ("Heartbeat", serde_json::Value::Null),
        TaskKind::Kicked { reason } => ("Kicked", serde_json::json!({ "reason": reason })),
    };

    let output = serde_json::json!({
        "kind": kind,
        "agent_name": agent_name,
        "response_file": response_file.display().to_string(),
        "content": content
    });

    serde_json::to_string_pretty(&output).unwrap_or_default()
}
```

#### 2.5: Add import for `AgentTransport`

**File:** `crates/agent_pool_cli/src/main.rs`

```rust
use agent_pool::{
    AgentTransport, Task, TaskKind,  // NEW
    // ... existing imports ...
};
```

---

### Task 3: Update test agents to use `AgentTransport`

**Goal:** Replace custom polling loops in test agents with `AgentTransport`.

#### 3.1: Rewrite `TestAgent` in agent_pool tests

**File:** `crates/agent_pool/tests/common/mod.rs`

The current `TestAgent` spawns a thread with a polling loop. We need to rewrite it to use `AgentTransport`.

Before (conceptual):
```rust
pub struct TestAgent {
    running: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<Vec<String>>>,
    ready_rx: Option<mpsc::Receiver<()>>,
}

impl TestAgent {
    pub fn start<F>(root: &Path, agent_id: &str, processing_delay: Duration, processor: F) -> Self
    where F: Fn(&str, &str) -> String + Send + 'static
    {
        // ... spawns thread with polling loop ...
    }
}
```

After:
```rust
pub struct TestAgent {
    handle: Option<thread::JoinHandle<Vec<String>>>,
    ready_rx: Option<mpsc::Receiver<()>>,
    stop_tx: Option<mpsc::Sender<()>>,
}

impl TestAgent {
    pub fn start<F>(root: &Path, agent_id: &str, processing_delay: Duration, processor: F) -> Self
    where F: Fn(&str, &str) -> String + Send + 'static
    {
        let root = root.to_path_buf();
        let agent_id = agent_id.to_string();

        let (ready_tx, ready_rx) = mpsc::sync_channel::<()>(0);
        let (stop_tx, stop_rx) = mpsc::channel::<()>();

        let handle = thread::spawn(move || {
            let transport = AgentTransport::new(&root, &agent_id)
                .expect("Failed to create transport");

            let mut processed_tasks = Vec::new();
            let mut first_message = true;

            loop {
                // Check for stop signal (non-blocking)
                if stop_rx.try_recv().is_ok() {
                    break;
                }

                // Wait for task (blocking, but will unblock when stop_tx is dropped
                // because the transport's directory gets deleted)
                let task = match transport.wait_for_task() {
                    Ok(t) => t,
                    Err(_) => break,  // Transport error, likely stopped
                };

                // Signal ready after first message
                if first_message {
                    first_message = false;
                    let _ = ready_tx.send(());
                }

                // Handle task based on kind
                let response = match &task.kind {
                    TaskKind::Heartbeat => "{}".to_string(),
                    TaskKind::Kicked { .. } => break,
                    TaskKind::Work { content } => {
                        thread::sleep(processing_delay);
                        let content_str = content.to_string();
                        processed_tasks.push(content_str.clone());
                        processor(&content_str, &agent_id)
                    }
                };

                if let Err(_) = transport.write_response(&response) {
                    break;
                }
            }

            processed_tasks
        });

        Self {
            handle: Some(handle),
            ready_rx: Some(ready_rx),
            stop_tx: Some(stop_tx),
        }
    }

    pub fn wait_ready(&mut self) {
        if let Some(rx) = self.ready_rx.take() {
            rx.recv().expect("Agent exited before signaling readiness");
        }
    }

    pub fn stop(mut self) -> Vec<String> {
        // Signal stop
        drop(self.stop_tx.take());

        // Wait for thread to finish
        self.handle.take()
            .expect("Agent already stopped")
            .join()
            .expect("Agent thread panicked")
    }
}
```

**Complication:** The `AgentTransport::wait_for_task()` blocks on the notify channel. When we want to stop the test agent, we need a way to unblock it. Options:
1. Delete the agent directory (causes watch error)
2. Use `recv_timeout` instead of `recv` and periodically check stop flag
3. Send a special "stop" file event

Option 1 is cleanest - dropping `stop_tx` and then removing the agent directory will cause the watcher to error, unblocking the thread.

#### 3.2: Update `GsdTestAgent` similarly

**File:** `crates/gsd_config/tests/common/mod.rs`

Apply the same pattern as Task 3.1.

---

### Task 4: Ensure daemon cleanup order is correct

**Goal:** Verify and document that the daemon deletes files in the correct order.

The daemon currently (in `io.rs` lines 446-447):
```rust
let _ = fs::remove_file(agent_path.join(TASK_FILE));
let _ = fs::remove_file(agent_path.join(RESPONSE_FILE));
```

This is the correct order:
1. Delete task.json first
2. Delete response.json second

This ensures agents waiting for `task.exists() && !response.exists()` will see either:
- Both exist (not ready)
- Task deleted, response exists (not ready)
- Both deleted (not ready, waiting for new task)
- New task exists, no response (ready!)

**No code change needed**, but add a comment:

```rust
// IMPORTANT: Delete task file BEFORE response file.
// This ensures agents watching for "task && !response" don't see
// a stale task after response is deleted but before task is deleted.
let _ = fs::remove_file(agent_path.join(TASK_FILE));
let _ = fs::remove_file(agent_path.join(RESPONSE_FILE));
```

---

### Task 5: Add unit tests for `AgentTransport`

**Goal:** Test the transport in isolation.

**File:** `crates/agent_pool/src/agent/transport.rs`

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn wait_for_task_returns_immediately_if_task_exists() {
        let tmp = TempDir::new().unwrap();
        let pool_root = tmp.path();

        // Pre-create task file
        let agent_dir = pool_root.join(AGENTS_DIR).join("test-agent");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(agent_dir.join(TASK_FILE), r#"{"kind": "Heartbeat"}"#).unwrap();

        let transport = AgentTransport::new(pool_root, "test-agent").unwrap();
        let task = transport.wait_for_task().unwrap();

        assert!(matches!(task.kind, TaskKind::Heartbeat));
    }

    #[test]
    fn wait_for_task_blocks_until_task_written() {
        let tmp = TempDir::new().unwrap();
        let pool_root = tmp.path().to_path_buf();

        let transport = AgentTransport::new(&pool_root, "test-agent").unwrap();
        let agent_dir = pool_root.join(AGENTS_DIR).join("test-agent");

        // Spawn thread to write task after delay
        let agent_dir_clone = agent_dir.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            fs::write(agent_dir_clone.join(TASK_FILE), r#"{"kind": "Task", "task": {}}"#).unwrap();
        });

        let task = transport.wait_for_task().unwrap();
        assert!(matches!(task.kind, TaskKind::Work { .. }));
    }

    #[test]
    fn write_response_waits_for_task_deletion() {
        let tmp = TempDir::new().unwrap();
        let pool_root = tmp.path().to_path_buf();

        let transport = AgentTransport::new(&pool_root, "test-agent").unwrap();
        let agent_dir = pool_root.join(AGENTS_DIR).join("test-agent");
        let task_file = agent_dir.join(TASK_FILE);
        let response_file = agent_dir.join(RESPONSE_FILE);

        // Write initial task
        fs::write(&task_file, r#"{"kind": "Task"}"#).unwrap();

        // Spawn thread to delete task after delay (simulating daemon)
        let task_file_clone = task_file.clone();
        let response_file_clone = response_file.clone();
        thread::spawn(move || {
            thread::sleep(Duration::from_millis(50));
            fs::remove_file(&task_file_clone).unwrap();
            fs::remove_file(&response_file_clone).unwrap();
        });

        // This should block until task is deleted
        transport.write_response("{}").unwrap();

        assert!(!task_file.exists());
    }
}
```

---

## Open Questions

1. **Typestate for compile-time safety?**
   - Pro: Impossible to call `write_response` without first getting a task
   - Con: More complex API, harder to use in tests
   - Recommendation: Start without typestate, add later if bugs occur

2. **Graceful stop mechanism for test agents?**
   - Current plan: Drop stop channel + delete agent directory
   - Alternative: Use `recv_timeout` and poll stop flag
   - Recommendation: Try the channel+delete approach first

3. **Should `AgentTransport` handle retries?**
   - If file read fails transiently, should it retry?
   - Recommendation: No, surface errors to caller. Keep transport simple.

4. **Thread safety of `AgentTransport`?**
   - Current design: `&self` methods (shared reference)
   - The `notify` watcher is internally thread-safe
   - `mpsc::Receiver` is `!Sync` but we only use it from one thread
   - Recommendation: Keep as-is, document that it's single-threaded

---

## Implementation Order

1. **Task 1** - Create `AgentTransport` (foundation)
2. **Task 5** - Add unit tests (verify it works)
3. **Task 2** - Update CLI (first real usage)
4. **Task 3** - Update test agents (fix the flaky tests)
5. **Task 4** - Document daemon cleanup order (defensive)

## Success Criteria

- [ ] No `thread::sleep` in agent waiting code
- [ ] Test agents use same code path as CLI agents
- [ ] All existing tests pass
- [ ] No flaky test failures from race conditions
