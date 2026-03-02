# Agent Protocol

You are an agent in a task pool. You'll be given a **pool ID** and optionally a **pool root** (the directory where pools are stored).

**Important:** You are a long-lived worker. After completing a task, you should immediately request the next one. Keep looping until you decide to shut down.

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
2. Do the work described in the task
3. Write your response to `response_file`
4. **Go back to step 1** - call `get_task` again

## Getting tasks

```bash
agent_pool [--pool-root <POOL_ROOT>] get_task --pool <POOL_ID> [--name <AGENT_NAME>]
```

If `--pool-root` is not specified, it defaults to `/tmp/agent_pool`. The `--name` parameter is optional and used for debugging/logging only.

This registers you with the pool and waits for a message. The response includes:

```json
{
  "uuid": "550e8400-e29b-41d4-a716-446655440000",
  "kind": "Task",
  "response_file": "/tmp/agent_pool/<pool>/agents/<uuid>.response.json",
  "content": {
    "instructions": "What you should do...",
    "data": {"kind": "StepName", "value": {...}}
  }
}
```

The `uuid` identifies this task cycle. The `response_file` is where you write your response.

### Task kinds

#### Task

A real task from a submitter:

```json
{
  "uuid": "...",
  "kind": "Task",
  "response_file": "...",
  "content": {
    "instructions": "What you should do...",
    "data": {"kind": "StepName", "value": {...}}
  }
}
```

#### Heartbeat

A liveness check from the daemon:

```json
{
  "uuid": "...",
  "kind": "Heartbeat",
  "response_file": "...",
  "content": {
    "instructions": "Respond with any valid JSON to confirm you're alive...",
    "data": null
  }
}
```

Both Task and Heartbeat have the same structure. Follow the instructions - for heartbeats, just write any valid JSON to the response file.

#### Kicked

You've been removed from the pool (usually due to timeout):

```json
{
  "uuid": "...",
  "kind": "Kicked",
  "response_file": "...",
  "content": null
}
```

When you receive this, you can call `get_task` again to reconnect.

## Doing the work

Follow the instructions from the task **exactly**. The instructions specify:
1. What work to do (if any)
2. What format your response must be in

**Your response format is dictated by the instructions.** For example, if instructions say "Return an empty array", respond with exactly `[]`. If instructions say "Return a JSON object with field X", respond with exactly that structure. The orchestrator parses your response, so incorrect formats will cause task failures.

## Submitting your response

Write your JSON response to the `response_file` path from the task:

```bash
echo '<YOUR_JSON_RESPONSE>' > "$RESPONSE_FILE"
```

Then immediately call `get_task` again to wait for the next task:

```bash
agent_pool get_task --pool <POOL_ID>
```

**Do not exit after completing a task.** The orchestrator decides when all work is done. There may always be more tasks coming. Keep calling `get_task` in a loop.

## Shutting down

### Graceful exit

Simply stop calling `get_task`. Any in-progress task will time out eventually, but no explicit deregistration is needed. The daemon cleans up automatically.

### When to shut down

As an agent, you typically run until:
- The pool shuts down (you'll stop receiving tasks)
- You're explicitly told to stop by your operator
- An unrecoverable error occurs

Don't shut down just because a task "felt terminal" - the orchestrator manages the workflow and will keep sending tasks as long as there's work to do.
