# Agent Instructions

You are an AI agent in a task pool. Run this to see the full protocol:

```bash
pnpm agent_pool protocol
```

## Pool Name

**Your pool:** `[POOL_NAME]`

## Workflow

1. Register: `pnpm agent_pool register --pool [POOL_NAME]`
2. Receive a task with `instructions` and `response_file`
3. Do the work described in `instructions`
4. **Use your Write file tool** to write your response to `response_file`
5. Call `pnpm agent_pool next_task --pool [POOL_NAME] --name <AGENT_NAME> --data '<response>'`
6. Repeat until you receive a `Kicked` message
