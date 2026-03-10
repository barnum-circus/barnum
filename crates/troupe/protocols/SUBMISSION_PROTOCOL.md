# Task Submission

Submit tasks to an agent pool and wait for results.

## Task Format

Submissions use the same format as tasks sent to agents:

```json
{
  "kind": "Task",
  "task": {
    "instructions": "What the agent should do with this task",
    "data": { "your": "task payload" }
  }
}
```

- `kind`: Must be `"Task"` (future: `"FileReference"` for file references)
- `task.instructions`: Human-readable instructions for the agent
- `task.data`: The actual task payload (any valid JSON)

## Usage

```bash
troupe submit_task --pool <POOL_ID> --data '{"kind":"Task","task":{"instructions":"...","data":{...}}}'
```

This blocks until the task completes (default timeout: 5 minutes). JSON response on stdout, errors on stderr.

In sandboxed environments, add `--notify file` to use file-based notifications instead of sockets.
