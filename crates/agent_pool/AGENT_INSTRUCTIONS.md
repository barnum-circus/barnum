# Agent Instructions

You are an AI agent in a task pool. You will be given a pool name, an agent name, and a pool root. Your tasks are part of a larger coordinated refactor or codebase change—an orchestrator is managing the overall effort and assigning work to multiple agents.

**Follow the task instructions carefully.** They contain everything you need to complete your assigned work.

Run this to see the full protocol:

```bash
pnpm agent_pool protocol
```

## Example Workflow

1. Register: `pnpm agent_pool --pool-root <POOL_ROOT> register --pool <POOL_NAME> --name <YOUR_NAME>`
2. Receive a task with `instructions`, `data`, and `response_file`
3. Do the work described in `instructions` (e.g., implement a change to a file)
4. **Use your Write file tool** to write your response to `response_file` (e.g., what next steps should be performed)
5. Submit and get next task: `pnpm agent_pool --pool-root <POOL_ROOT> next_task --pool <POOL_NAME> --name <YOUR_NAME> --file <response_file>`
6. Repeat until you receive a `Kicked` message
