# Transport Abstraction Plan

## Overview

Communication between components involves two orthogonal choices:

1. **Payload format:** How is the content delivered?
   - **Inline:** The message contains the actual content
   - **File reference:** The message contains a path; recipient reads the file

2. **Notification mechanism:** How does the recipient know there's a message?
   - **Socket:** RPC call (direct connection, immediate)
   - **FS events:** Write a file to a watched location; recipient notices via file watcher

This 2x2 grid applies to both:
- **Submission:** Submitter → Daemon (and response back)
- **Task handling:** Daemon → Agent (and response back)

## The 2x2 Grid

|                    | Inline                          | File Reference                     |
|--------------------|---------------------------------|------------------------------------|
| **Socket**         | Send content over socket        | Send path over socket; recipient reads file |
| **FS Events**      | Write content to notification file | Write path to notification file; recipient reads referenced file |

### Why FS Events Exist

Socket = RPC. Direct call to daemon.

FS events = fallback when sockets are blocked (sandboxed environments). Create a file in a watched directory to trigger an event. The daemon's file watcher notices and processes it.

### The "Double Read" with FS Events + File Reference

When using FS events notification with file reference payload:

1. Submitter writes `pending/<uuid>/task.json` containing `{"file": "/path/to/actual/task.json"}`
2. Daemon reads `pending/<uuid>/task.json` (read #1) → gets the path
3. Daemon reads `/path/to/actual/task.json` (read #2) → gets the content

This is not waste—the first read answers "what event happened?" and the second gets the actual content. The notification file is tiny (just metadata + path).

With socket notification, there's only one read because the RPC call itself carries the path.

## Applies To Both Directions

### Submission (Submitter ↔ Daemon)

```
Submitter                 Daemon
   |                         |
   |---- submit task ------->|   (notification + payload)
   |                         |
   |<--- response -----------|   (notification + payload)
   |                         |
```

**CLI flags:**
- `--data "content"` = inline payload
- `--file /path/to/task.json` = file reference payload
- `--notify socket` (default) or `--notify file` = notification mechanism

### Task Handling (Daemon ↔ Agent)

```
Daemon                    Agent
   |                         |
   |---- dispatch task ----->|   (notification + payload)
   |                         |
   |<--- response -----------|   (notification + payload)
   |                         |
```

**Agent commands:**
- `register` - Initial connection, receives first task (health check)
- `next_task` - Submit response to previous task, receive next task

**CLI flags (same as submission):**
- `--data "response"` = inline payload
- `--file /path/to/response.json` = file reference payload
- `--notify socket` (default) or `--notify file` = notification mechanism

**Agent loop:**
```bash
task=$(agent_pool register --pool $POOL --name $NAME)
while true; do
    response=$(process "$task")
    task=$(agent_pool next_task --pool $POOL --name $NAME --data "$response")
done
```

After `register`, every `next_task` call is symmetric: "here's my response, give me next task."

## Sandbox Restrictions

Sandboxes block the `connect()` syscall, which blocks socket notification.

**What sandboxes block:** Socket notification (can't do RPC)
**What sandboxes allow:** FS events notification (just file read/write)

The payload format (inline vs file reference) is unaffected by sandbox restrictions. A sandboxed agent can use either payload format—they just can't use socket notification.

| Environment | Socket Notification | FS Events Notification |
|-------------|---------------------|------------------------|
| Normal      | ✓                   | ✓                      |
| Sandboxed   | ✗ (blocked)         | ✓                      |

## Current Implementation

Currently we conflate these concepts:

```rust
// "submit" = socket notification + inline payload
pub fn submit(root: &Path, input: &str) -> io::Result<Response>;

// "submit_file" = fs events notification + inline payload
// (misleading name—it's about notification, not payload format)
pub fn submit_file(root: &Path, input: &str) -> io::Result<Response>;
```

The current `--file` flag in the CLI reads the file locally and sends content—it doesn't send a file reference.

**Socket submission is broken:** The client side (`submit()`) sends data and waits for response. The daemon side (`accept_socket_task()`) reads the data but then drops the connection without ever sending a response. Clients hang forever.

## Target Design

Separate the two axes clearly:

```rust
/// What content are we sending?
enum Payload {
    Inline(String),
    FileReference(PathBuf),
}

/// How do we communicate with the daemon?
/// Used identically for submission and agent responses.
enum NotifyMethod {
    Socket { socket_path: PathBuf },
    FsEvents { dir: PathBuf },
}

impl NotifyMethod {
    /// Send message to daemon, wait for response.
    /// Same logic for submission and agent task completion.
    fn send_and_receive(&self, payload: &Payload) -> io::Result<String> {
        match self {
            Self::Socket { socket_path } => {
                // Connect to socket, write payload, read response
            }
            Self::FsEvents { dir } => {
                // Write payload to dir/task.json
                // Poll for dir/response.json
                // Read and return response
            }
        }
    }
}
```

**CLI:**
```bash
# Submission
agent_pool submit_task --pool $POOL --data "content"              # inline
agent_pool submit_task --pool $POOL --file /path/to/task.json     # file reference
agent_pool submit_task --pool $POOL --data "content" --notify file  # sandboxed

# Agent registration (first call, gets initial task/health check)
agent_pool register --pool $POOL --name $NAME

# Agent next task (submit response, get next task)
agent_pool next_task --pool $POOL --name $NAME --data "response"
agent_pool next_task --pool $POOL --name $NAME --file /path/to/response.json
agent_pool next_task --pool $POOL --name $NAME --data "response" --notify file  # sandboxed
```

**Unified flag names:**
- `--data "content"` = inline payload (same name for submission and agent response)
- `--file /path` = file reference payload
- `--notify socket|file` = notification mechanism (default: socket)

---

## Future: Auto-Discovery of Notification Mechanism

Currently, users must explicitly pass `--notify file` in sandboxed environments. Auto-discovery could detect when sockets are blocked and fall back automatically.

**Options (all deferred):**
1. **Try socket, fall back to file** - latency cost on every call
2. **Cache in environment variable** - `AGENT_POOL_NOTIFY=file`
3. **Cache in pool directory** - write `.notify-method` file
4. **Don't auto-discover** - user explicitly passes `--notify file`

For initial implementation, option 4 (explicit flag) is simplest.

---

## Future: Remote Pools

A third notification mechanism could be added for remote daemons:

| Notification     | Description                    | File Reference Works? |
|------------------|--------------------------------|----------------------|
| Local Socket     | Unix socket on same machine    | ✓ (shared filesystem) |
| FS Events        | File watcher on same machine   | ✓ (shared filesystem) |
| Remote Socket    | TCP socket to different machine| ✗ (no shared filesystem) |

For remote sockets with file reference payload, the CLI would read the file and send content inline (automatic fallback).

This is out of scope for now but the design accommodates it.

---

# Implementation Tasks

## Task 1: Fix Daemon Socket Response (daemon-side)

**Goal:** Make socket submission work end-to-end. Currently `submit()` sends data but daemon drops the connection without responding.

**Current state:**
- Client side works: `submit()` connects, sends length + content, waits for response
- Daemon side broken: `accept_socket_task()` reads content, then drops stream

**Subtasks:**

### 1.1: Add `Transport::Socket` Variant

**File:** `daemon/io.rs`

Add `Socket` variant to existing `Transport` enum:

```rust
pub(super) enum Transport {
    Directory(PathBuf),
    Socket(Stream),  // NEW
}
```

**Complication:** `Stream` from interprocess doesn't implement `Debug`. Options:
- Manual `Debug` impl that prints `"Socket(...)"`
- Wrapper type `SocketStream(Stream)` that implements Debug
- Use `#[derive(Debug)]` with `#[debug(skip)]` if available

### 1.2: Add `TransportMap::register_socket()`

**File:** `daemon/io.rs`

Current `register_directory()` assumes a path. Socket tasks don't have paths. Add a separate method:

```rust
impl<Id: TransportId> TransportMap<Id> {
    /// Register a socket-based transport (no path lookup needed).
    pub fn register_socket(&mut self, id: Id, stream: Stream, data: Id::Data) -> bool {
        match self.entries.entry(id) {
            Entry::Occupied(_) => false,
            Entry::Vacant(entry) => {
                entry.insert((Transport::Socket(stream), data));
                // Note: no path_to_id entry - socket tasks aren't discovered via fs
                true
            }
        }
    }
}
```

### 1.3: Update `accept_socket_task()` to Return Stream

**File:** `daemon/wiring.rs`

Change signature from returning just content to returning content + stream:

```rust
fn accept_socket_task(listener: &Listener) -> io::Result<Option<(String, Stream)>> {
    match listener.accept() {
        Ok(stream) => {
            // Read length-prefixed content
            let mut reader = BufReader::new(&stream);
            let mut len_line = String::new();
            reader.read_line(&mut len_line)?;
            let len: usize = len_line.trim().parse()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            let mut content = vec![0u8; len];
            reader.read_exact(&mut content)?;
            let content = String::from_utf8(content)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            // Get stream back from reader
            let stream = reader.into_inner();

            Ok(Some((content, stream)))
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
        Err(e) => Err(e),
    }
}
```

**Complication:** `BufReader::new(&stream)` borrows stream. Need to use `into_inner()` to get it back, or read without BufReader.

### 1.4: Register Socket Tasks in `io_loop`

**File:** `daemon/wiring.rs`

In the main loop, after accepting a socket task:

```rust
if let Some((content, stream)) = accept_socket_task(listener)? {
    let external_id = task_id_allocator.allocate_external();

    external_task_map.register_socket(
        external_id,
        stream,
        ExternalTaskData {
            content,
            timeout: io_config.default_task_timeout,
        },
    );

    let _ = events_tx.send(Event::TaskSubmitted {
        task_id: TaskId::External(external_id),
    });
}
```

### 1.5: Update `finish()` to Handle Both Transport Types

**File:** `daemon/io.rs`

The `finish()` method writes the response. Update to handle socket:

```rust
pub fn finish(&mut self, id: ExternalTaskId, response: &str) -> io::Result<()> {
    let (transport, _data) = self.entries.remove(&id)
        .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "task not found"))?;

    // Remove from path lookup if it was directory-based
    if let Transport::Directory(ref path) = transport {
        self.path_to_id.remove(path);
    }

    match transport {
        Transport::Directory(path) => {
            fs::write(path.join(RESPONSE_FILE), response)?;
        }
        Transport::Socket(mut stream) => {
            use std::io::Write;
            writeln!(stream, "{}", response.len())?;
            stream.write_all(response.as_bytes())?;
            stream.flush()?;
        }
    }

    Ok(())
}
```

### 1.6: Test End-to-End

After all subtasks, test:
```bash
# Start daemon
agent_pool start --pool test-pool &

# Submit via socket (currently hangs, should now work)
agent_pool submit_task --pool test-pool --input '{"task": "test"}'
```

---

## Task 2: CLI Flag for Inline Data (`--data`)

**Goal:** Rename `--input` to `--data` for consistency with the 2x2 model.

**Current state:**
- `submit_task --input "content"` sends inline content via socket
- `submit_task --file /path` reads file and sends content inline (misleading)

**Changes:**

### 2.1: Rename `--input` to `--data`

**File:** `agent_pool_cli/src/main.rs`

```rust
// Before
#[arg(long, conflicts_with = "file")]
input: Option<String>,

// After
#[arg(long, conflicts_with = "file")]
data: Option<String>,
```

Update all references from `input` to `data` in the match arms.

### 2.2: Update Help Text

Make help text clear about what `--data` means:

```rust
/// Task content as inline string (sent directly to daemon)
#[arg(long, conflicts_with = "file")]
data: Option<String>,
```

---

## Task 3: CLI Flag for Notification Mechanism (`--notify`)

**Goal:** Add `--notify socket|file` flag to choose between socket and fs-events notification.

**Current state:**
- `submit_task --input` always uses socket
- `submit_task --file` always uses fs-events (despite the name suggesting file reference)

**Changes:**

### 3.1: Add `--notify` Flag

**File:** `agent_pool_cli/src/main.rs`

```rust
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum NotifyMethod {
    #[default]
    Socket,
    File,
}

// In SubmitTask command:
/// Notification mechanism: socket (default) or file (for sandboxed environments)
#[arg(long, default_value = "socket")]
notify: NotifyMethod,
```

### 3.2: Update Submit Logic

**File:** `agent_pool_cli/src/main.rs`

```rust
Command::SubmitTask { pool, data, file, notify } => {
    let root = resolve_pool(&pool);

    // Get content (either inline or from file)
    let content = match (data, file) {
        (Some(d), None) => d,
        (None, Some(path)) => fs::read_to_string(&path)?,
        (None, None) => {
            eprintln!("Either --data or --file must be provided");
            return ExitCode::FAILURE;
        }
        _ => unreachable!(),
    };

    // Send via chosen notification method
    let result = match notify {
        NotifyMethod::Socket => submit(&root, &content),
        NotifyMethod::File => submit_file(&root, &content),
    };

    // ... handle result
}
```

### 3.3: Update Help Text

```rust
/// Notification mechanism: socket (default, faster) or file (works in sandboxed environments)
#[arg(long, default_value = "socket")]
notify: NotifyMethod,
```

---

## Task 4: File Reference Payload Support

**Goal:** Add ability to send a file path instead of content. The daemon reads the file.

**Current state:**
- `--file /path` reads the file client-side and sends content inline
- No way to say "here's a path, daemon should read it"

**Why this matters:**
- Large payloads: don't want to read into memory client-side
- Avoids double-copy: content stays on disk, daemon reads directly

### 4.1: Add Wire Format for File Reference

When client sends a file reference, it needs to be distinguishable from inline content. Options:

**Option A: JSON envelope**
```json
{"kind": "inline", "content": "..."}
{"kind": "file_ref", "path": "/path/to/task.json"}
```

**Option B: Length-prefix convention**
- Positive length = inline content follows
- Negative length = file path follows (absolute value is path length)

**Option C: Separate protocol command**
- `SUBMIT_INLINE <len>\n<content>`
- `SUBMIT_FILE <len>\n<path>`

**Recommendation:** Option A (JSON envelope) is clearest and most extensible.

### 4.2: Update Client to Send File Reference

**File:** `client/submit.rs`

Add new function or parameter:

```rust
pub fn submit_file_ref(root: &Path, file_path: &Path) -> io::Result<Response> {
    let envelope = json!({
        "kind": "file_ref",
        "path": file_path.display().to_string()
    });
    submit_raw(root, &envelope.to_string())
}
```

Or update existing `submit()` to take a `Payload` enum.

### 4.3: Update Daemon to Handle File Reference

**File:** `daemon/wiring.rs` or `daemon/io.rs`

When receiving a task, check if it's a file reference:

```rust
fn resolve_payload(raw: &str) -> io::Result<String> {
    let envelope: serde_json::Value = serde_json::from_str(raw)?;

    match envelope.get("kind").and_then(|k| k.as_str()) {
        Some("file_ref") => {
            let path = envelope.get("path")
                .and_then(|p| p.as_str())
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing path"))?;
            fs::read_to_string(path)
        }
        Some("inline") | None => {
            // Treat as inline content (backward compatible)
            envelope.get("content")
                .and_then(|c| c.as_str())
                .map(String::from)
                .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "missing content"))
        }
        Some(other) => {
            Err(io::Error::new(io::ErrorKind::InvalidData, format!("unknown kind: {other}")))
        }
    }
}
```

### 4.4: Update CLI

**File:** `agent_pool_cli/src/main.rs`

Change `--file` semantics:
- `--file /path` now sends a file reference (daemon reads the file)
- For "read file client-side, send inline," users can do `--data "$(cat /path)"`

Or add a new flag:
- `--file /path` = file reference (daemon reads)
- `--file-inline /path` = read client-side, send inline (current behavior)

**Recommendation:** Change `--file` to mean file reference. The old behavior is achievable via shell substitution.

### 4.5: Update File-Based Submission

**File:** `client/submit_file.rs`

Same changes for fs-events notification path. The `pending/<uuid>/task.json` can contain either:
- `{"kind": "inline", "content": "..."}`
- `{"kind": "file_ref", "path": "/path/to/task.json"}`

Daemon's `register_pending_task()` needs to call `resolve_payload()` when reading the task file.

---

## Task 5: Agent Commands (`register`, `next_task`)

**Goal:** Implement the agent-side CLI commands shown in the target design.

**Current state:**
- `get_task` polls for task.json, reads it, prints it
- No `register` or `next_task` commands
- No socket-based agent communication

### 5.1: Rename `get_task` to `register`

Or keep both with `register` as alias. First call creates agent directory and waits for first task.

### 5.2: Add `next_task` Command

Submit response from previous task, then wait for next task:

```rust
Command::NextTask { pool, name, data, file, notify } => {
    let root = resolve_pool(&pool);

    // Get response content
    let response = match (data, file) {
        (Some(d), None) => d,
        (None, Some(path)) => /* send file ref or read */,
        _ => /* error */,
    };

    // Write response to response.json (or send via socket)
    // Wait for next task
    // Print task
}
```

### 5.3: Socket-Based Agent Communication (deferred)

Currently agents use fs-based polling. Socket-based agent communication would be a larger change requiring:
- Agent connects to socket
- Daemon keeps connection open
- Tasks pushed to agent over persistent connection

This is a bigger architectural change. For now, agents use fs-based polling with `--notify file` being the only option.

---

## Summary: Deployment Order

These tasks can be deployed independently:

1. **Task 1** (daemon socket response) - fixes broken functionality, no CLI changes
2. **Task 2** (`--data` flag) - simple rename, backward compatible if `--input` kept as hidden alias
3. **Task 3** (`--notify` flag) - new flag with good default, non-breaking
4. **Task 4** (file reference) - breaking change to `--file` semantics, or add new flag
5. **Task 5** (agent commands) - new commands, non-breaking

Recommended order: 1 → 2 → 3 → 4 → 5

Task 1 is the most important because it fixes broken functionality. Tasks 2-3 are cleanup. Task 4 adds new capability. Task 5 is polish.
