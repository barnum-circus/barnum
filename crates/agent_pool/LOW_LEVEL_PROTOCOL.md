# Low-Level Protocol

> **Warning:** This documents the internal file and socket protocols. You should use the CLI commands (`submit_task`, `register`, `next_task`) instead. This documentation is for debugging, understanding internals, or implementing alternative clients.

## Directory Structure

A pool at `/tmp/gsd/<POOL_ID>/` has:

```
/tmp/gsd/<POOL_ID>/
├── daemon.lock          # PID file (daemon running indicator)
├── daemon.sock          # Unix socket for IPC (socket protocol)
├── status               # Empty file, signals daemon is ready
├── agents/              # One subdirectory per registered agent
│   └── <agent-name>/
│       ├── task.json    # Task from daemon → agent
│       └── response.json# Response from agent → daemon
└── pending/             # One file pair per task submission
    ├── <uuid>.request.json   # Submitted task
    └── <uuid>.response.json  # Result from agent
```

## File-Based Submission Protocol

Used by `submit_task --notify file`. Works in sandboxed environments where sockets are blocked.

### Submitting a Task

1. Generate a unique ID (UUID recommended)
2. Write task JSON to `pending/<id>.request.json`:
   ```json
   {"kind": "Task", "task": {"instructions": "...", "data": {...}}}
   ```
   Or with file reference (daemon reads content from path):
   ```json
   {"kind": "FileReference", "path": "/absolute/path/to/content.json"}
   ```

3. The daemon watches `pending/` and picks up the request

### Waiting for Response

1. Watch or poll for `pending/<id>.response.json`
2. Response format:
   ```json
   {"kind": "Processed", "stdout": "agent's response content"}
   ```
   Or on timeout:
   ```json
   {"kind": "NotProcessed", "reason": "Timeout"}
   ```

3. Clean up: The submitter is responsible for removing both files after reading

## Socket Protocol

Used by `submit_task --notify socket` (default). Faster but requires Unix socket access.

### Connection

Connect to `daemon.sock` Unix socket in the pool directory.

### Request Format

```
<length>\n<json>
```

Where:
- `<length>` is the byte length of the JSON payload (ASCII digits + newline)
- `<json>` is the task JSON (same format as file protocol)

Example:
```
68
{"kind": "Task", "task": {"instructions": "echo", "data": "hello"}}
```

### Response Format

Same framing as request:
```
<length>\n<json>
```

The JSON is the same `Processed` or `NotProcessed` response as the file protocol.

## Agent File Protocol

Used internally between daemon and agents.

### Registration

1. Agent creates directory: `agents/<agent-name>/`
2. Daemon detects directory creation via filesystem watcher
3. Daemon writes task (or heartbeat) to `agents/<agent-name>/task.json`

### Task Format (daemon → agent)

```json
{
  "kind": "Task",
  "task": {"instructions": "...", "data": {...}}
}
```

Or heartbeat:
```json
{
  "kind": "Heartbeat",
  "task": {"instructions": "Respond with any valid JSON...", "data": null}
}
```

Or kicked (agent timed out):
```json
{
  "kind": "Kicked",
  "reason": "Timeout"
}
```

### Response Format (agent → daemon)

Agent writes response to `agents/<agent-name>/response.json`:
- Any valid JSON for heartbeats
- Task-specific format for real tasks (usually the agent's output)

### Lifecycle

1. Agent creates directory → daemon assigns task → agent writes response
2. Daemon clears both files, writes next task
3. Repeat until agent is kicked or deregisters

### Deregistration

Agent removes its directory, or daemon writes `Kicked` message and removes directory.

## Timing and Atomicity

### Atomic Writes

All file writes should be atomic (write to temp file, then rename) to prevent partial reads. The daemon uses `/tmp/gsd-*.tmp` for atomic writes.

### Filesystem Watching

The daemon uses:
- **macOS:** FSEvents via `notify` crate
- **Linux:** inotify via `notify` crate

Events are typically delivered within milliseconds. The daemon does not poll.

### Race Conditions

- Request files are processed in arrival order (no priority)
- Agent directory creation races with task assignment are handled by the daemon
- Multiple agents compete fairly for tasks (deterministic selection based on agent ID)

## Status Signals

- `daemon.lock` exists and contains valid PID → daemon is running
- `status` file exists → daemon is ready to accept tasks and agents
- Agent directory exists → agent is registered (may be idle or busy)

## Error Handling

- Malformed JSON in request files is ignored (no response written)
- Agents that don't respond within timeout are kicked
- Socket connections that close unexpectedly are cleaned up
- File-based submissions that timeout get `NotProcessed` response
