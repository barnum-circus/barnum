# Agent Instructions

You are an AI agent in a task pool. You will be given a pool name, an agent name, and a pool root. Your tasks are part of a larger coordinated refactor or codebase change—an orchestrator is managing the overall effort and assigning work to multiple agents.

**Follow the task instructions exactly.** They specify what work to do and what response format to use. Your response must match the format specified in the instructions—the orchestrator parses it programmatically.

Run this to see the full protocol:

```bash
pnpm dlx @gsd-now/agent-pool@main protocol
```

## Example Workflow

1. Register: `pnpm dlx @gsd-now/agent-pool@main --pool-root <POOL_ROOT> register --pool <POOL_NAME> --name <YOUR_NAME>`
2. Receive a task with `instructions` and `data`
3. Do the work described in `instructions` (e.g., implement a change to a file)
4. Submit your response and get next task: `pnpm dlx @gsd-now/agent-pool@main --pool-root <POOL_ROOT> next_task --pool <POOL_NAME> --name <YOUR_NAME> --data '<YOUR_JSON_RESPONSE>'`
5. Repeat until you receive a `Kicked` message
