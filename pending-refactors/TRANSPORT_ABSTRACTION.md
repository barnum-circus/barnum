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
- `--input "content"` = inline payload
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

Same 2x2 applies. Agent responses can be:
- `--result "content"` = inline payload
- `--file /path/to/response.json` = file reference payload (or write to `response_file` path)

Notification mechanism for agent responses:
- Socket: agent calls `complete_task` which sends over socket
- FS events: agent writes to `response_file`, daemon watches for it

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

## Target Design

Separate the two axes clearly:

```rust
// Payload format
enum Payload {
    Inline(String),
    FileReference(PathBuf),
}

// Notification mechanism
enum Notification {
    Socket,
    FsEvents,
}

// Submission with explicit choices
fn submit(root: &Path, payload: Payload, notify: Notification) -> io::Result<Response>;
```

**CLI:**
```bash
# Socket + inline (fastest, default)
agent_pool submit_task --pool $POOL --input "content"

# Socket + file reference (avoids copying large content)
agent_pool submit_task --pool $POOL --file /path/to/task.json

# FS events + inline (for sandboxed submitters)
agent_pool submit_task --pool $POOL --input "content" --notify file

# FS events + file reference (sandboxed + large content)
agent_pool submit_task --pool $POOL --file /path/to/task.json --notify file
```

## Independent Migrations

The two axes can be migrated independently:

### Payload Format Migration

Add `--file` flag that sends a path (file reference) instead of reading and inlining content. Works with either notification mechanism.

**Before:** `--file` reads file, sends content (inline)
**After:** `--file` sends the path (file reference)

### Notification Mechanism Migration

Already exists (`submit` vs `submit_file`), but rename for clarity:
- `--notify socket` (default)
- `--notify file`

### Migration Order

Either order works:
1. Fix `--file` to be true file reference, then add `--notify` flag
2. Add `--notify` flag first, then fix `--file` semantics

## Edge Case: Inaccessible Filesystems

Currently we assume the submitter, daemon, and agents all share the same filesystem. File reference works because everyone can read the path.

In a hypothetical future where the daemon can't access the submitter's filesystem:
- File reference wouldn't work directly
- The CLI would need to detect this and fall back to reading the file and sending inline

This is similar to the "remote socket" case discussed below. For now, we assume shared filesystem access.

**Implementation note:** If we ever need to handle this case, the CLI (not the daemon) should detect it and automatically convert file reference to inline. The daemon shouldn't need to know about this complexity.

---

## Implementation Phases

### Phase 1: Clarify Payload Format

1. Rename current `--file` behavior to something explicit (it's inline, not file reference)
2. Add true file reference support: `--file` sends the path, daemon reads the file
3. Update both submission and task completion to support both payload formats

### Phase 2: Clarify Notification Mechanism

1. Rename `submit()` and `submit_file()` to reflect notification mechanism
2. Add `--notify socket|file` CLI flag
3. Default to socket, fall back to file in sandboxed environments

### Phase 3: Daemon Traits (Internal Refactor)

Abstract the notification mechanism behind traits so daemon code doesn't branch on it:

```rust
trait SubmissionReceiver {
    fn recv(&mut self) -> io::Result<Submission>;
}

trait AgentChannel {
    fn dispatch(&mut self, task: &Task) -> io::Result<()>;
    fn poll_response(&mut self) -> io::Result<Option<String>>;
}
```

Implementations for socket and file-based notification are separate; daemon logic is unified.

---

## Dependencies

**Phase 1 and 2 can start now** - no async runtime needed.

**Phase 3 (daemon traits)** can happen in parallel or after.

**Async conversion** (if desired) requires DAEMON_REFACTOR.md to be completed first.

---

## Future: Remote Pools

A third notification mechanism could be added for remote daemons:

| Notification     | Description                    | File Reference Works? |
|------------------|--------------------------------|----------------------|
| Local Socket     | Unix socket on same machine    | ✓ (shared filesystem) |
| FS Events        | File watcher on same machine   | ✓ (shared filesystem) |
| Remote Socket    | TCP socket to different machine| ✗ (no shared filesystem) |

For remote sockets with file reference payload, the CLI would read the file and send content inline (automatic fallback). From the user's perspective, `--file` always works—the CLI handles the details.

This is out of scope for now but the design accommodates it.
