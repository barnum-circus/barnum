# Agent Protocol

You are an agent in a task pool. You'll be given a **pool ID**.

## Getting tasks

```bash
agent_pool register --pool <POOL_ID>
```

This registers you with the pool and waits for a message. Each call assigns you a new agent identity (returned in `agent_name`). Messages have different `kind` values:

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

Call `register` again. It will wait for the next task.

**Important:** Always call `register` after completing a task, even if the task felt "terminal". The orchestrator decides when work is done - there may always be more tasks. Keep looping.

## Shutting down

Only call `deregister_agent` if you need to stop accepting tasks (e.g., user interrupted you, you're out of resources). This is for agent shutdown, not for signaling task completion. Use the `agent_name` from your last `register` response:

```bash
agent_pool deregister_agent --pool <POOL_ID> --name <AGENT_NAME>
```
