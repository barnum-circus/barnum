# Agent Protocol

You are an agent in a task pool. You'll be given a **pool name**, an **agent name**, and optionally a **root** (the directory where pools are stored).

**Important:** You are a long-lived worker. After completing a task, immediately request the next one. Keep looping until shutdown.

## The agent loop

```
┌─────────────────────────────────────────┐
│                                         │
│   ┌──────────────┐                      │
│   │  get_task    │◄─────────────────┐   │
│   └──────┬───────┘                  │   │
│          │                          │   │
│          ▼                          │   │
│   ┌──────────────┐                  │   │
│   │  do work     │                  │   │
│   └──────┬───────┘                  │   │
│          │                          │   │
│          ▼                          │   │
│   ┌──────────────┐                  │   │
│   │ write resp   │──────────────────┘   │
│   └──────────────┘                      │
│                                         │
└─────────────────────────────────────────┘
```

1. Call `get_task` to wait for work
2. Do everything in the task message
3. Write your response to `response_file`
4. **Go back to step 1** - call `get_task` again

## Getting tasks

```bash
pnpm dlx @barnum/troupe get_task --pool <POOL_NAME> --name <AGENT_NAME>
```

If you need a custom root (not the default `/tmp/troupe`):

```bash
pnpm dlx @barnum/troupe get_task --pool <POOL_NAME> --name <AGENT_NAME> --root <ROOT>
```

This blocks until a message is available. The response is JSON:

```json
{
  "uuid": "550e8400-e29b-41d4-a716-446655440000",
  "kind": "Task",
  "response_file": "/path/to/response.json",
  "content": {
    "instructions": "What you should do...",
    "data": {"kind": "StepName", "value": {...}}
  }
}
```

## Message kinds

### Task

A real task from a submitter. Do everything in the message and write your response to `response_file`.

### Heartbeat

A liveness check. Write any valid JSON (like `{}`) to `response_file` and continue.

### Kicked

You've been removed from the pool. Kill the `get_task` process and exit.

## Submitting your response

Write your JSON response to the `response_file` path:

```bash
echo '<YOUR_JSON_RESPONSE>' > "$RESPONSE_FILE"
```

Then immediately call `get_task` again.

## Shutting down

Kill the `get_task` process and exit. The daemon cleans up automatically.
