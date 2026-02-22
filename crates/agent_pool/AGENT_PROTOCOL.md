# Agent Protocol

You are an agent in a task pool. You'll be given a **pool ID** and your **agent name**.

## Getting tasks

```bash
agent_pool get_task --pool <POOL_ID> --name <YOUR_NAME>
```

This registers you with the pool and waits until a task is available. When a task arrives, it prints JSON:

```json
{
  "kind": "Task",
  "response_file": "/tmp/gsd/<POOL_ID>/agents/<YOUR_NAME>/response.json",
  "content": {
    "task": {"kind": "StepName", "value": {...}},
    "instructions": "What you should do..."
  }
}
```

## Doing the work

Follow the instructions from the task. The instructions will tell you what valid responses look like.

## Writing your response

Use your **Write tool** (not bash) to write your response to `response_file`. The format depends on the task's instructions.

**Important:** Always use the Write file tool, not bash commands like `echo`. Bash file operations may trigger permission prompts that interrupt the workflow.

## Getting the next task

Call `get_task` again. It will wait for the next task.

**Important:** Always call `get_task` after completing a task, even if the task felt "terminal". The orchestrator decides when work is done - there may always be more tasks. Keep looping.

## Shutting down

Only call `deregister_agent` if you need to stop accepting tasks (e.g., user interrupted you, you're out of resources). This is for agent shutdown, not for signaling task completion:

```bash
agent_pool deregister_agent --pool <POOL_ID> --name <YOUR_NAME>
```
