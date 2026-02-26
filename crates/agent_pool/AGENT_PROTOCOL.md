# Agent Protocol

You are an agent in a task pool. You'll be given a **pool ID** and optionally a **pool root** (the directory where pools are stored).

## Getting tasks

```bash
agent_pool [--pool-root <POOL_ROOT>] register --pool <POOL_ID> --name <AGENT_NAME>
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

Follow the instructions from the task. The instructions will tell you what valid responses look like.

## Writing your response

Use your **Write tool** (not bash) to write your response to `response_file`. The format depends on the task's instructions.

**Important:** Always use the Write file tool, not bash commands like `echo`. Bash file operations may trigger permission prompts that interrupt the workflow.

## Getting the next task

After writing your response to `response_file`, call `next_task` to submit it and wait for the next task:

```bash
agent_pool [--pool-root <POOL_ROOT>] next_task --pool <POOL_ID> --name <AGENT_NAME> --file <RESPONSE_FILE>
```

**Important:** Always use `--file` (not `--data`) to avoid permission prompts. Always call `next_task` after completing a task, even if the task felt "terminal". The orchestrator decides when work is done - there may always be more tasks. Keep looping.

## Shutting down

### Graceful exit (after submitting final response)

Use `--deregister` to submit your response and exit cleanly without waiting for the next task:

```bash
agent_pool [--pool-root <POOL_ROOT>] next_task --pool <POOL_ID> --name <AGENT_NAME> --file <RESPONSE_FILE> --deregister
```

This waits for the daemon to acknowledge your response before deregistering.

### Abort (without submitting a response)

If you need to stop immediately without submitting a response (e.g., user interrupted you, out of resources):

```bash
agent_pool [--pool-root <POOL_ROOT>] deregister_agent --pool <POOL_ID> --name <AGENT_NAME>
```

This is for emergency shutdown only. Any in-progress task will fail.
