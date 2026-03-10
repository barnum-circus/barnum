# Agent Protocol

How agents interact with the pool to receive tasks, send heartbeats, and write responses.

For the JSON format that agents receive and must return, see [Task Format](task-format.md). For how tasks get submitted to the pool, see [Submission Protocol](submission-protocol.md).

## Overview

Agents are long-lived workers that loop forever:

```
┌──────────────────────────────────────────────┐
│  get_task ──→ process task ──→ write response │
│      ↑                              │         │
│      └──────────────────────────────┘         │
└──────────────────────────────────────────────┘
```

Each iteration uses a fresh anonymous worker UUID. Agents have no persistent identity across tasks.

## Getting a Task

Agents call the `get_task` CLI command to request work:

```bash
troupe get_task --pool my-pool
```

This blocks until the daemon assigns work. The response is JSON on stdout:

```jsonc
{
  // Unique worker ID for this task cycle
  "uuid": "550e8400-e29b-41d4-a716-446655440000",

  // One of: "Task", "Heartbeat", "Kicked"
  "kind": "Task",

  // Where to write the response when done
  "response_file": "/tmp/troupe/pools/my-pool/agents/550e8400.response.json",

  // The task payload (see Task Format doc for details)
  "content": {
    "task": {
      "kind": "AnalyzeFile",
      "value": { "file": "src/main.rs" }
    },
    "instructions": "...",
    "timeout_seconds": 300
  }
}
```

## Task Types

### Task

Real work from a submitter. Process it and write the response.

```jsonc
{ "kind": "Task", ... }
```

### Heartbeat

A liveness check from the daemon. Respond with any valid JSON (e.g., `"ok"` or `{}`). If you don't respond in time, the daemon assumes you're dead and kicks you.

```jsonc
{ "kind": "Heartbeat", ... }
```

### Kicked

The daemon removed you — usually because you timed out on a heartbeat. The `get_task` call returns this instead of blocking. Just call `get_task` again to reconnect with a fresh UUID.

```jsonc
{
  "kind": "Kicked",
  "reason": "Timeout"
}
```

When you receive a Kicked response, there is no `response_file` — there's nothing to respond to.

## Writing Responses

After processing a task or heartbeat, write the response to the `response_file` path from the `get_task` output:

```bash
echo '[{"kind": "NextStep", "value": {"result": "done"}}]' > "$RESPONSE_FILE"
```

The response is the agent's stdout — a JSON array of next tasks (see [Task Format](task-format.md)).

After writing the response, immediately call `get_task` again to get the next task.

## Under the Hood: File Protocol

Each `get_task` call creates a fresh anonymous worker with a UUID. The interaction uses flat files in the pool's `agents/` directory:

```
<pool>/agents/
├── <uuid>.ready.json      # Agent → Daemon: "I'm available"
├── <uuid>.task.json       # Daemon → Agent: "Here's work"
└── <uuid>.response.json   # Agent → Daemon: "Here's the result"
```

**Lifecycle of one task:**

1. Agent generates a UUID and writes `<uuid>.ready.json` (contains `{"name": "agent_name"}`)
2. Agent watches for `<uuid>.task.json` to appear
3. Daemon spots the ready file, assigns a task, writes `<uuid>.task.json`
4. Agent reads the task, processes it, writes `<uuid>.response.json`
5. Daemon reads the response, cleans up all files for this UUID
6. Agent generates a new UUID and starts over

The `get_task` CLI command abstracts all of this — agents don't need to manage UUIDs or file paths directly.

## Timeouts

If an agent doesn't respond to a task (or heartbeat) within the configured timeout, the daemon:

1. Writes a `Kicked` message to the agent
2. Returns `NotProcessed { reason: "timeout" }` to the submitter
3. The submitter (Barnum) applies the retry policy

Agents that get kicked can reconnect immediately by calling `get_task` again.

## Example: Minimal Agent Script

```bash
#!/bin/bash
# A simple agent that processes tasks in a loop

POOL="my-pool"

while true; do
  # Get next task (blocks until work available)
  RESPONSE=$(troupe get_task --pool "$POOL")

  KIND=$(echo "$RESPONSE" | jq -r '.kind')
  RESPONSE_FILE=$(echo "$RESPONSE" | jq -r '.response_file')

  case "$KIND" in
    Task)
      TASK=$(echo "$RESPONSE" | jq '.content')
      # ... do work with $TASK ...
      RESULT='[]'  # or array of next tasks
      echo "$RESULT" > "$RESPONSE_FILE"
      ;;
    Heartbeat)
      echo '"ok"' > "$RESPONSE_FILE"
      ;;
    Kicked)
      # Reconnect on next iteration
      ;;
  esac
done
```
