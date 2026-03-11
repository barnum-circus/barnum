# Submission Protocol

How tasks get submitted to the agent pool and how responses come back.

For the JSON format that agents receive, see [Task Format](task-format.md). For how agents interact with the pool, see [Agent Protocol](agent-protocol.md).

## Overview

Submitters (like the `barnum` CLI) send tasks to the daemon, which dispatches them to available agents. There are two transport mechanisms:

| | Socket (default) | File-based (fallback) |
|---|---|---|
| **Mechanism** | Unix domain socket | Files in `submissions/` directory |
| **Speed** | Faster | Slightly slower |
| **Sandbox-safe** | No (sockets may be blocked) | Yes |
| **Flag** | `--notify socket` | `--notify file` |

## Submitting via CLI

```bash
# Inline payload
troupe submit_task \
  --pool my-pool \
  --data '{"kind":"Task","task":{"instructions":"Do something","data":{"file":"main.rs"}}}' \
  --notify socket

# Payload from file (avoids shell escaping issues)
troupe submit_task \
  --pool my-pool \
  --file /path/to/payload.json \
  --notify file

# With timeout
troupe submit_task \
  --pool my-pool \
  --data '...' \
  --timeout-secs 300
```

The CLI blocks until the agent responds (or timeout), then prints the response JSON to stdout.

## Payload Format

The submission payload is the JSON that the agent will receive as `content` in its `get_task` response. For Barnum, this is:

```jsonc
{
  "task": {
    "kind": "StepName",
    "value": { "file": "src/main.rs" }
  },
  "instructions": "Auto-generated markdown...",
  "timeout_seconds": 300
}
```

See [Task Format](task-format.md) for full details.

## Response Format

The CLI outputs the daemon's response as JSON:

**Success**: agent processed the task:
```jsonc
{
  "kind": "Processed",
  "stdout": "[{\"kind\": \"NextStep\", \"value\": {}}]"
}
```

The `stdout` field contains the agent's response (a JSON array of next tasks, as a string).

**Not processed**: agent timed out or pool stopped:
```jsonc
{
  "kind": "NotProcessed",
  "reason": "timeout"
}
```

Possible reasons: `"timeout"`, `"stopped"`.

## How Barnum Uses This

Barnum wraps the submission protocol to add workflow semantics. For each task:

1. **Build payload**: generates instructions from the step config (schemas, valid transitions, isolation preamble)
2. **Submit**: calls `troupe submit_task` via CLI
3. **Parse response**: extracts `stdout` from `Processed` response
4. **Validate**: checks that returned tasks are valid transitions with valid schemas
5. **Retry**: on timeout, error, or invalid response, applies the step's retry policy

```
Barnum Runner             Troupe Daemon                  Agent
    │                            │                         │
    │── submit_task ────────────→│                         │
    │                            │── task.json ───────────→│
    │                            │                         │── process
    │                            │←── response.json ───────│
    │←── Processed {stdout} ─────│                         │
    │                            │                         │
    │── validate + route next    │                         │
```

## Under the Hood: File-Based Protocol

When using `--notify file`, submissions use flat files:

```
<pool>/submissions/
├── <uuid>.request.json     # Submitter → Daemon
└── <uuid>.response.json    # Daemon → Submitter
```

**Request payload** wraps content in a transport envelope:

Inline:
```jsonc
{
  "kind": "Inline",
  "content": "{\"task\": {...}, \"instructions\": \"...\", \"timeout_seconds\": 300}"
}
```

File reference (content lives in a separate file):
```jsonc
{
  "kind": "FileReference",
  "path": "/absolute/path/to/content.json"
}
```

**Lifecycle:**

1. Submitter generates a UUID
2. Writes `<uuid>.request.json` to `submissions/` (via atomic write through `scratch/`)
3. Daemon detects the new file, reads payload, dispatches to an available agent
4. Agent processes task, writes response
5. Daemon writes `<uuid>.response.json`
6. Submitter reads response, cleans up both files

## Under the Hood: Socket Protocol

When using `--notify socket`, the submitter connects to `<pool>/daemon.sock`:

**Request framing:** length-prefixed JSON
```
<byte_length>\n<json_payload>
```

**Response:** same framing
```
<byte_length>\n<json_response>
```

The socket protocol avoids filesystem overhead and is preferred when not running in a sandbox.

## Pool Directory Structure

```
<pool>/
├── daemon.lock              # PID file (prevents multiple daemons)
├── daemon.sock              # Unix socket for submissions
├── status                   # Empty file; signals daemon is ready
├── agents/                  # Agent worker files (see Agent Protocol)
│   ├── <uuid>.ready.json
│   ├── <uuid>.task.json
│   └── <uuid>.response.json
├── submissions/             # File-based submissions
│   ├── <uuid>.request.json
│   └── <uuid>.response.json
└── scratch/                 # Temporary files for atomic writes
```
