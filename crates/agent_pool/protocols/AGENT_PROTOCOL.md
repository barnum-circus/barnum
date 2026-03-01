# Agent Protocol

You are an agent in a task pool. You'll be given a **pool ID** and optionally a **pool root** (the directory where pools are stored).

## Getting tasks

```bash
pnpm dlx @gsd-now/agent-pool@main [--pool-root <POOL_ROOT>] register --pool <POOL_ID> --name <AGENT_NAME>
```

If `--pool-root` is not specified, it defaults to `/tmp/agent_pool`.

This registers you with the pool and waits for a message. Messages have different `kind` values:

### Task

A real task from a submitter:

```json
{
  "kind": "Task",
  "task": {
    "instructions": "What you should do...",
    "data": {"kind": "StepName", "value": {...}}
  }
}
```

### Heartbeat

A liveness check from the daemon:

```json
{
  "kind": "Heartbeat",
  "task": {
    "instructions": "Respond with any valid JSON to confirm you're alive...",
    "data": null
  }
}
```

Both Task and Heartbeat have the same structure under `task`. Follow the instructions - for heartbeats, just write any valid JSON to the response file.

### Kicked

You've been removed from the pool (usually due to timeout):

```json
{
  "kind": "Kicked",
  "reason": "Timeout"
}
```

When you receive this, exit gracefully. Re-registering with `register` will fail until the pool restarts.

## Doing the work

Follow the instructions from the task **exactly**. The instructions specify:
1. What work to do (if any)
2. What format your response must be in

**Your response format is dictated by the instructions.** For example, if instructions say "Return an empty array", respond with exactly `[]`. If instructions say "Return a JSON object with field X", respond with exactly that structure. The orchestrator parses your response, so incorrect formats will cause task failures.

## Getting the next task

After completing your work, call `next_task` to submit your response and wait for the next task:

```bash
pnpm dlx @gsd-now/agent-pool@main [--pool-root <POOL_ROOT>] next_task --pool <POOL_ID> --name <AGENT_NAME> --data '<YOUR_JSON_RESPONSE>'
```

**Important:** Always use `--data` to pass your response directly. Always call `next_task` after completing a task, even if the task felt "terminal". The orchestrator decides when work is done - there may always be more tasks. Keep looping.

## Shutting down

### Graceful exit (after submitting final response)

Use `--deregister` to submit your response and exit cleanly without waiting for the next task:

```bash
pnpm dlx @gsd-now/agent-pool@main [--pool-root <POOL_ROOT>] next_task --pool <POOL_ID> --name <AGENT_NAME> --data '<YOUR_JSON_RESPONSE>' --deregister
```

This waits for the daemon to acknowledge your response before deregistering.

### Abort (without submitting a response)

If you need to stop immediately without submitting a response (e.g., user interrupted you, out of resources):

```bash
pnpm dlx @gsd-now/agent-pool@main [--pool-root <POOL_ROOT>] deregister_agent --pool <POOL_ID> --name <AGENT_NAME>
```

This is for emergency shutdown only. Any in-progress task will fail.
