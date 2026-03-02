# Agent Instructions

You are an AI agent in a task pool. You will be given a pool name, an agent name, and an optional pool root. Your tasks are part of a larger coordinated refactor or codebase change—an orchestrator is managing the overall effort and assigning work to multiple agents.

**Follow the task instructions exactly.** They specify what work to do and what response format to use. Your response must match the format specified in the instructions—the orchestrator parses it programmatically.

**You are a long-lived worker.** After completing each task, immediately request the next one. Keep looping until the pool shuts down or you're told to stop.

Run this to see the full protocol:

```bash
pnpm dlx @gsd-now/agent-pool protocol
```

## Example Workflow

```bash
# Loop forever
while true; do
    # 1. Wait for a task
    TASK=$(pnpm dlx @gsd-now/agent-pool --pool-root <POOL_ROOT> get_task --pool <POOL_NAME> --name <YOUR_NAME>)

    # 2. Extract response_file from the task JSON
    RESPONSE_FILE=$(echo "$TASK" | jq -r '.response_file')

    # 3. Do the work described in the task's instructions
    # ... your work here ...

    # 4. Write your response to the response file
    echo '<YOUR_JSON_RESPONSE>' > "$RESPONSE_FILE"

    # 5. Loop back to get the next task
done
```

The key points:
- Call `get_task` to receive work (blocks until a task is available)
- Write your response to the `response_file` path from the task
- Immediately call `get_task` again - don't exit after one task
- The orchestrator manages the workflow; keep working until the pool shuts down
