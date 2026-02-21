# Agent Protocol

This document describes how agents communicate with the multiplexer daemon.

## Directory Structure

```
<root>/
  daemon.lock      # PID of running daemon
  daemon.sock      # Unix socket for task submission
  agents/
    <agent-id>/    # One directory per agent
      next_task    # Written by daemon when assigning work
      output       # Written by agent when work is complete
```

## Registering as an Agent

Create a directory under `<root>/agents/` with your agent ID:

```bash
mkdir -p /path/to/root/agents/my-agent
```

The daemon watches this directory and will automatically register your agent.

## Receiving Tasks

When the daemon has work for you, it writes the task content to `next_task`:

```
<root>/agents/<your-id>/next_task
```

Poll for this file or watch for its creation.

## Completing Tasks

When you've finished processing a task:

1. Write your result to `output`
2. **Do NOT delete `next_task`** - the daemon handles cleanup

```bash
# Read the task
task=$(cat /path/to/root/agents/my-agent/next_task)

# Do your work...
result="your result here"

# Write output (daemon will clean up both files)
echo "$result" > /path/to/root/agents/my-agent/output
```

## Recovery

If your agent crashes mid-task, the `next_task` file remains. A replacement agent with the same ID can read it and resume work.

## Complete Example

```bash
#!/bin/bash
AGENT_DIR="/path/to/root/agents/my-agent"
mkdir -p "$AGENT_DIR"

while true; do
    if [ -f "$AGENT_DIR/next_task" ]; then
        task=$(cat "$AGENT_DIR/next_task")

        # Process the task (replace with your logic)
        result="Processed: $task"

        # Write output - daemon will delete both files
        echo "$result" > "$AGENT_DIR/output"
    fi
    sleep 0.1
done
```

## Protocol Summary

| Step | Actor | Action |
|------|-------|--------|
| 1 | Agent | Create `agents/<id>/` directory |
| 2 | Daemon | Detect agent, mark as available |
| 3 | Daemon | Write task to `next_task` |
| 4 | Agent | Read `next_task`, process |
| 5 | Agent | Write result to `output` |
| 6 | Daemon | Read `output`, delete both files, send response to submitter |
