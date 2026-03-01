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

**From a file (works in sandboxed environments):**
```bash
agent_pool submit_task --pool <POOL_ID> --file /path/to/task.json
```

**Inline JSON (faster, requires socket access):**
```bash
agent_pool submit_task --pool <POOL_ID> --data '{"kind":"Task","task":{"instructions":"...","data":{...}}}'
```

Both methods block until the task completes (default timeout: 5 minutes).

## When to use which

- Use `--file` in sandboxed environments where Unix sockets are blocked
- Use `--data` for quick submissions when you have full system access

The output is the same either way: JSON response on stdout, errors on stderr.
