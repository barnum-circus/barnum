# Notify-Based Agent Waiting

## Goal

Replace sleep-based polling with `notify`-based file watching for agents waiting for tasks. Test agents should call the CLI binary (`get_task`, `next_task`) rather than implementing their own file-based polling in Rust.

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

### Socket vs File Communication

**Sockets are NOT currently used for agent task retrieval.** The socket mechanism is only used for:
- Task submission (`submit_task` command via `submit()`)

Agent-side operations are entirely file-based:
- `get_task` / `register` - polls for `task.json` (file-based)
- `next_task` - writes `response.json`, polls for next task (file-based)

**TODO:** Add `--notify socket` option to agent commands (`get_task`, `next_task`) for environments where sockets are available. This would allow the daemon to push tasks to agents instead of agents polling.

### Existing Daemon Abstractions

**Event loop (wiring.rs) - IoEvent enum:**
```rust
enum IoEvent {
    Fs(notify::Event),      // File system changes
    Socket(String, Stream), // Socket connections (for task submission)
    Effect(Effect),         // Internal effects from state machine
}

// Single unified channel, blocking recv
loop {
    match io_rx.recv() {
        Ok(IoEvent::Fs(event)) => handle_fs_event(...),
        Ok(IoEvent::Socket(raw, stream)) => handle_socket(...),
        Ok(IoEvent::Effect(effect)) => execute_effect(...),
        Err(_) => break, // Channel closed = shutdown
    }
}
```

Key insight: **The daemon uses a notify watcher + unified event channel pattern.** It doesn't poll. The agent side should use the same pattern.

### Current Polling Implementations (Problem)

**CLI (`crates/agent_pool_cli/src/main.rs:195-239`):**
```rust
fn wait_for_task(...) -> Result<String, String> {
    loop {
        if task_file.exists() && !response_file.exists() {
            return Ok(...);
        }
        thread::sleep(Duration::from_millis(100));  // POLLING!
    }
}
```

**Test agents (`crates/gsd_config/tests/common/mod.rs`):**
```rust
// "running" is an Arc<AtomicBool> used for graceful shutdown.
// When stop() is called, it sets running to false.
// The loop checks this flag on each iteration.
while running_clone.load(Ordering::SeqCst) {
    if task_file.exists() && !response_file.exists() {
        // Process task...
    }
    thread::sleep(Duration::from_millis(10));  // POLLING!
}
```

The `running` flag exists because test agents need to be stoppable. When the test calls `agent.stop()`, it sets the flag to `false`, and the agent thread exits its loop gracefully.

## Target Architecture

### Key Insight: Event-Driven, Same Code Path

1. The CLI's `get_task` and `next_task` commands should use `notify` instead of polling
2. Test agents should spawn the CLI binary as a subprocess (same code path!)
3. The test agent's "running flag" becomes irrelevant when using subprocesses - just kill the subprocess

### Architecture Diagram

```
CLI Binary (agent_pool get_task)
================================

┌─────────────────┐
│  notify watcher │
│  (watches agent │
│   directory)    │
└────────┬────────┘
         │ fs events
         ▼
┌─────────────────┐
│  Event Loop     │
│  (blocks on     │
│   recv())       │
└────────┬────────┘
         │
         │ task ready?
         ▼
┌─────────────────┐
│  Read & Output  │
│  task.json      │
└─────────────────┘


Test Agent (subprocess-based)
=============================

┌─────────────────┐
│  Test starts    │
│  subprocess:    │
│  agent_pool     │
│  get_task       │
└────────┬────────┘
         │
         │ stdout
         ▼
┌─────────────────┐
│  Test parses    │
│  JSON output    │
└────────┬────────┘
         │
         │ process & respond
         ▼
┌─────────────────┐
│  Test calls:    │
│  agent_pool     │
│  next_task      │
└─────────────────┘
```

## Concrete Tasks

### Task 1: Move Transport to Shared Module

**Goal:** Make `Transport` accessible to both daemon and agent code.

**File:** `crates/agent_pool/src/daemon/io.rs` → `crates/agent_pool/src/transport.rs`

The daemon's `Transport` enum already has `read()` and `write()` methods. Agents will use the same enum:

```rust
// Agent code uses Transport the same way as daemon code
let transport = Transport::Directory(agent_dir);
let task = transport.read(TASK_FILE)?;
// ... process task ...
transport.write(RESPONSE_FILE, &response)?;
```

For socket support later, the methods will just do socket I/O (ignoring the filename parameter):
- `transport.read(_)` on Socket → reads next message from socket
- `transport.write(_, content)` on Socket → sends message to socket

**Steps:**

1.1. Create new file `crates/agent_pool/src/transport.rs` with the `Transport` enum and its impl (moved from `daemon/io.rs`).

1.2. Update `crates/agent_pool/src/lib.rs`:
```rust
mod transport;
pub use transport::Transport;
```

1.3. Update `crates/agent_pool/src/daemon/io.rs` to use `crate::Transport` instead of local definition.

### Task 2: Create Agent Event Loop Module

**Goal:** Create `notify`-based waiting functions for agents using `Transport`.

**File:** `crates/agent_pool/src/agent.rs` (new file)

```rust
//! Agent-side event loop for waiting on tasks.

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::io;
use std::path::Path;
use std::sync::mpsc;

use crate::{Transport, RESPONSE_FILE, TASK_FILE};

/// Events the agent cares about.
pub enum AgentEvent {
    /// A file changed in the agent directory.
    FileChanged,
    /// The watcher encountered an error.
    WatchError(notify::Error),
}

/// Create a watcher for a directory-based transport.
///
/// Returns the watcher (keep alive) and a receiver for events.
/// For socket-based transports, this would be a no-op (sockets are already event-driven).
pub fn create_watcher(
    transport: &Transport,
) -> io::Result<Option<(RecommendedWatcher, mpsc::Receiver<AgentEvent>)>> {
    let Some(dir) = transport.path() else {
        // Socket transport - no filesystem watcher needed
        return Ok(None);
    };

    let (tx, rx) = mpsc::channel();

    let mut watcher = RecommendedWatcher::new(
        move |res: Result<notify::Event, notify::Error>| match res {
            Ok(_) => { let _ = tx.send(AgentEvent::FileChanged); }
            Err(e) => { let _ = tx.send(AgentEvent::WatchError(e)); }
        },
        Config::default(),
    )
    .map_err(io::Error::other)?;

    watcher
        .watch(dir, RecursiveMode::NonRecursive)
        .map_err(io::Error::other)?;

    Ok(Some((watcher, rx)))
}

/// Check if a task is ready to be processed (file-based only).
pub fn is_task_ready(transport: &Transport) -> bool {
    let Some(dir) = transport.path() else {
        // Socket transport - task readiness is handled by blocking read
        return false;
    };
    let task_file = dir.join(TASK_FILE);
    let response_file = dir.join(RESPONSE_FILE);
    task_file.exists() && !response_file.exists()
}

/// Wait for a task to be ready (file-based transports).
///
/// The condition `task.exists() && !response.exists()` handles all cases:
/// - After writing response: keeps waiting until daemon cleans up and assigns new task
/// - Fresh start: waits for first task assignment
///
/// For socket-based transports, just call `transport.read()` which blocks.
pub fn wait_for_task(
    transport: &Transport,
    events_rx: &mpsc::Receiver<AgentEvent>,
) -> io::Result<()> {
    if is_task_ready(transport) {
        return Ok(());
    }

    loop {
        match events_rx.recv() {
            Ok(AgentEvent::FileChanged) => {
                if is_task_ready(transport) {
                    return Ok(());
                }
            }
            Ok(AgentEvent::WatchError(e)) => {
                return Err(io::Error::other(e));
            }
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "watcher channel closed",
                ));
            }
        }
    }
}
```

### Task 3: Update CLI to Use Transport and Agent Event Loop

**Goal:** Replace polling in CLI with Transport-based event-driven waiting.

**File:** `crates/agent_pool_cli/src/main.rs`

**Before:**
```rust
fn wait_for_task(...) -> Result<String, String> {
    loop {
        if task_file.exists() && !response_file.exists() {
            return Ok(...);
        }
        thread::sleep(Duration::from_millis(100));  // POLLING!
    }
}
```

**After:**
```rust
use agent_pool::{Transport, create_watcher, wait_for_task, TASK_FILE, RESPONSE_FILE};

fn run_agent(pool_root: &Path, name: &str) -> Result<String, String> {
    let agent_dir = pool_root.join(AGENTS_DIR).join(name);
    fs::create_dir_all(&agent_dir).map_err(|e| e.to_string())?;

    let transport = Transport::Directory(agent_dir);

    // Set up watcher (returns None for socket transport)
    let watcher_and_rx = create_watcher(&transport).map_err(|e| e.to_string())?;
    let (_watcher, events_rx) = watcher_and_rx
        .ok_or("socket transport not yet supported")?;

    // Wait for task using notify (no polling!)
    wait_for_task(&transport, &events_rx).map_err(|e| e.to_string())?;

    // Read task using Transport
    let raw = transport.read(TASK_FILE).map_err(|e| e.to_string())?;

    // ... parse envelope, build output JSON ...
}

fn handle_next_task(transport: &Transport, events_rx: &Receiver<AgentEvent>, response: &str) -> Result<String, String> {
    // Write response using Transport
    transport.write(RESPONSE_FILE, response).map_err(|e| e.to_string())?;

    // Wait for next task (handles cleanup transition automatically)
    wait_for_task(transport, events_rx).map_err(|e| e.to_string())?;

    // Read next task
    transport.read(TASK_FILE).map_err(|e| e.to_string())
}
```

The `next_task` command just writes the response and calls `wait_for_task` - no explicit cleanup wait needed. The `task.exists() && !response.exists()` condition handles the transition automatically.

### Task 4: Update Test Agents to Call CLI Binary

**Goal:** Test agents should spawn `agent_pool get_task` and `agent_pool next_task` as subprocesses instead of reimplementing file-based polling in Rust.

**File:** `crates/gsd_config/tests/common/mod.rs`

**Before:**
```rust
pub struct GsdTestAgent {
    running: Arc<AtomicBool>,  // For graceful shutdown
    handle: Option<thread::JoinHandle<Vec<String>>>,
    ready_rx: Option<mpsc::Receiver<()>>,
}

// Thread that polls filesystem directly
while running_clone.load(Ordering::SeqCst) {
    if task_file.exists() && !response_file.exists() {
        // Process task...
    }
    thread::sleep(Duration::from_millis(10));  // POLLING!
}
```

**After:**
```rust
use std::process::{Child, Command, Stdio};

pub struct GsdTestAgent {
    child: Option<Child>,  // Subprocess running agent_pool get_task
    // No more running flag - just kill the subprocess
}

impl GsdTestAgent {
    pub fn start<F>(root: &Path, agent_id: &str, processor: F) -> Self
    where
        F: Fn(&str) -> String + Send + 'static,
    {
        // Spawn subprocess: agent_pool get_task --pool <root> --name <agent_id>
        let mut child = Command::new("agent_pool")
            .args(["get_task", "--pool", &root.display().to_string(), "--name", agent_id])
            .stdout(Stdio::piped())
            .spawn()
            .expect("Failed to spawn agent_pool");

        // Spawn thread to read stdout and process tasks
        let stdout = child.stdout.take().unwrap();
        thread::spawn(move || {
            // Read JSON output from get_task
            // Process and call next_task with response
            // Repeat until subprocess exits or is killed
        });

        Self { child: Some(child) }
    }

    pub fn stop(mut self) -> Vec<String> {
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();  // Graceful shutdown = just kill the subprocess
            let _ = child.wait();
        }
        vec![]  // Or collect processed tasks some other way
    }
}
```

**Benefits:**
- Test agents use the exact same code path as real agents
- No more `running` flag complexity - subprocess shutdown is clean
- Any bugs in `get_task` / `next_task` are caught by tests

### Task 5: Audit All Sleeps and Timeouts

**Goal:** Search for all `thread::sleep`, `recv_timeout`, `Duration::from_millis`, etc. and validate each usage is intentional.

**Search patterns:**
```bash
rg "thread::sleep" --type rust
rg "recv_timeout" --type rust
rg "Duration::from_millis" --type rust
rg "Duration::from_secs" --type rust
```

**Expected legitimate uses:**
- Daemon config timeouts (idle agent timeout, task timeout)
- Socket accept thread polling (10ms) - acceptable since it's I/O bound, not CPU
- Test delays for simulating slow agents

**Should be removed/replaced:**
- CLI `wait_for_task` polling (→ notify)
- Test agent filesystem polling (→ subprocess calling CLI)
- Any other filesystem polling

## Implementation Order

1. **Task 1: Move Transport** - Mechanical refactor, no behavior change
2. **Task 2: Create agent module** - New code using Transport, can be tested in isolation
3. **Task 3: Update CLI** - Use Transport and agent module, existing tests verify it works
4. **Task 4: Update test agents** - Make tests use CLI subprocess
5. **Task 5: Audit sleeps** - Final cleanup pass

## Notes

- The socket mechanism for task submission is separate and unrelated to agent waiting
- Test agents calling the CLI binary ensures they exercise the same code path as real agents
- The `running` flag pattern becomes unnecessary when test agents are subprocesses - just kill the process to stop
