# Agent Protocol

You are a long-lived agent in a task pool. After completing a task, immediately request the next one. Keep looping until shutdown.

## Getting tasks

```bash
pnpm dlx @barnum/troupe get_task --name <AGENT_NAME> [--pool <POOL_NAME>] [--root <ROOT>]
```

- `--pool` defaults to `default`. Do not guess — if you weren't given a pool name, omit it.
- `--root` defaults to `/tmp/troupe`.

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

- **Task**: Real work. Do everything in the message and write your response to `response_file`.
- **Heartbeat**: Liveness check. Write any valid JSON (like `{}`) to `response_file` and continue.
- **Kicked**: You've been removed from the pool. Kill the `get_task` process and exit.

## Submitting your response

Write your JSON response to the `response_file` path, then immediately call `get_task` again.

## Shutting down

Kill the `get_task` process and exit. The daemon cleans up automatically.
