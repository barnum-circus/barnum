# Agent Protocol

This document describes how to implement an agent for the agent pool.

## Your Agent Directory

Create a directory for your agent:

```bash
mkdir -p /path/to/root/agents/my-agent
```

Your agent directory contains three files (managed by you and the pool):

```
agents/my-agent/
  next_task     # Task appears here when work is assigned
  in_progress   # You rename next_task here to claim it
  output        # You write your result here
```

## Receiving Tasks

Poll for `next_task` or watch for its creation. When it appears, a task has been assigned to you.

## Processing Tasks

When you see `next_task`:

1. **Atomically rename** `next_task` to `in_progress` to claim the task
2. Read the task content from `in_progress`
3. Process the task
4. Write your result to `output`
5. Delete `in_progress`

The atomic rename prevents race conditions - if the rename fails, another process already claimed the task.

```bash
if mv "$AGENT_DIR/next_task" "$AGENT_DIR/in_progress" 2>/dev/null; then
    task=$(cat "$AGENT_DIR/in_progress")

    # Process the task...
    result="your result here"

    echo "$result" > "$AGENT_DIR/output"
    rm -f "$AGENT_DIR/in_progress"
fi
```

## Complete Example

```bash
#!/bin/bash
AGENT_DIR="/path/to/root/agents/my-agent"
mkdir -p "$AGENT_DIR"

while true; do
    if [ -f "$AGENT_DIR/next_task" ]; then
        if mv "$AGENT_DIR/next_task" "$AGENT_DIR/in_progress" 2>/dev/null; then
            task=$(cat "$AGENT_DIR/in_progress")

            # Your processing logic here
            result="Processed: $task"

            echo "$result" > "$AGENT_DIR/output"
            rm -f "$AGENT_DIR/in_progress"
        fi
    fi
    sleep 0.1
done
```

## Recovery

If your agent crashes mid-task:
- `in_progress` will still exist with the task content
- You can resume by reading `in_progress` on restart
- Write to `output` and delete `in_progress` to complete
