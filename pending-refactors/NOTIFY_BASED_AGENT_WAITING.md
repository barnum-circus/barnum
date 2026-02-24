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
- `next_task` - writes `response.json`, waits for cleanup, polls for next task (file-based)

This is intentional: file-based communication works in sandboxed environments where sockets are blocked.

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

### Task 1: Create Agent Event Loop Module

**Goal:** Create `notify`-based waiting functions for agents.

**File:** `crates/agent_pool/src/agent.rs` (new file)

```rust
//! Agent-side event loop for waiting on tasks.

use notify::{Config, RecommendedWatcher, RecursiveMode, Watcher};
use std::io;
use std::path::Path;
use std::sync::mpsc;

use crate::{RESPONSE_FILE, TASK_FILE};

/// Events the agent cares about.
pub enum AgentEvent {
    /// A file changed in the agent directory.
    FileChanged,
    /// The watcher encountered an error.
    WatchError(notify::Error),
}

/// Create a watcher for an agent directory.
pub fn create_agent_watcher(
    agent_dir: &Path,
) -> io::Result<(RecommendedWatcher, mpsc::Receiver<AgentEvent>)> {
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
        .watch(agent_dir, RecursiveMode::NonRecursive)
        .map_err(io::Error::other)?;

    Ok((watcher, rx))
}

/// Check if a task is ready to be processed.
pub fn is_task_ready(agent_dir: &Path) -> bool {
    let task_file = agent_dir.join(TASK_FILE);
    let response_file = agent_dir.join(RESPONSE_FILE);
    task_file.exists() && !response_file.exists()
}

/// Wait for a task to be ready (blocks until ready).
pub fn wait_for_task(
    agent_dir: &Path,
    events_rx: &mpsc::Receiver<AgentEvent>,
) -> io::Result<()> {
    if is_task_ready(agent_dir) {
        return Ok(());
    }

    loop {
        match events_rx.recv() {
            Ok(AgentEvent::FileChanged) => {
                if is_task_ready(agent_dir) {
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

/// Wait for task.json to be deleted (daemon acknowledged response).
pub fn wait_for_cleanup(
    agent_dir: &Path,
    events_rx: &mpsc::Receiver<AgentEvent>,
) -> io::Result<()> {
    let task_file = agent_dir.join(TASK_FILE);

    if !task_file.exists() {
        return Ok(());
    }

    loop {
        match events_rx.recv() {
            Ok(AgentEvent::FileChanged) => {
                if !task_file.exists() {
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

### Task 2: Update CLI to Use Agent Event Loop

**Goal:** Replace polling in CLI with event-driven waiting.

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
use agent_pool::{create_agent_watcher, wait_for_task, wait_for_cleanup, is_task_ready};

fn wait_for_task_event_driven(agent_dir: &Path, name: &str) -> Result<String, String> {
    let (watcher, events_rx) = create_agent_watcher(agent_dir)
        .map_err(|e| e.to_string())?;
    let _watcher = watcher; // Keep alive

    wait_for_task(agent_dir, &events_rx).map_err(|e| e.to_string())?;

    // Read and return task (same as before)
    let task_file = agent_dir.join(TASK_FILE);
    let response_file = agent_dir.join(RESPONSE_FILE);
    // ... parse envelope, build output JSON ...
}
```

Also update `next_task` command to use `wait_for_cleanup` followed by `wait_for_task`.

### Task 3: Update Test Agents to Call CLI Binary

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

### Task 4: Audit All Sleeps and Timeouts

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

1. **Task 1: Create agent module** - New code, can be tested in isolation
2. **Task 2: Update CLI** - Use new code, existing tests verify it works
3. **Task 3: Update test agents** - Make tests use CLI subprocess
4. **Task 4: Audit sleeps** - Final cleanup pass

## Notes

- The socket mechanism for task submission is separate and unrelated to agent waiting
- Test agents calling the CLI binary ensures they exercise the same code path as real agents
- The `running` flag pattern becomes unnecessary when test agents are subprocesses - just kill the process to stop
