# Quick Start

This guide will get you up and running with GSD in a few minutes.

## Prerequisites

- Node.js 18+ (for running via `pnpm dlx` or `npx`)
- One or more Claude instances (or other LLM agents)

## Step 1: Start the Agent Pool

In one terminal, start the agent pool daemon:

```bash
pnpm dlx @gsd-now/agent-pool@main start --pool agents
```

This creates a pool named "agents" that will coordinate work between your agents.

## Step 2: Set Up Your Agents

Pass this information to one or more Claude instances:

```
You are an AI agent in a task pool. You will be given a pool name, an agent name, and an optional pool root. Your tasks are part of a larger coordinated refactor or codebase change—an orchestrator is managing the overall effort and assigning work to multiple agents.

**Follow the task instructions exactly.** They specify what work to do and what response format to use. Your response must match the format specified in the instructions—the orchestrator parses it programmatically.

Run this to see the full protocol:

pnpm dlx @gsd-now/agent-pool@main protocol

---

Your name is c1. The pool name is agents.
```

Each agent should have a unique name (c1, c2, c3, etc.) for debugging purposes.

## Step 3: Get a Config File

Download a demo config:

```bash
curl -O https://raw.githubusercontent.com/rbalicki2/gsd/main/crates/gsd_cli/demos/linear/config.jsonc
```

Or create your own. Run this to see the JSON schema:

```bash
pnpm dlx @gsd-now/gsd@main config schema
```

## Step 4: Run the Workflow

Start the GSD workflow:

```bash
pnpm dlx @gsd-now/gsd@main run config.jsonc --pool agents --initial-state '[{"kind": "Start", "value": {}}]'
```

GSD will dispatch tasks to your agents, and they'll work through the state machine you defined.

## Next Steps

- Explore the [demo configs](https://github.com/rbalicki2/gsd/tree/main/crates/gsd_cli/demos) for more examples
- Read about [creating config files](/docs/)
- Check out the [Rust API documentation](/docs/) for advanced usage
