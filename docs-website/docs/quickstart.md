# Quick Start

This guide walks you through running your first GSD workflow with Claude agents.

## What is GSD?

GSD orchestrates AI agents through type-safe workflows. You define a state machine in JSON, and GSD dispatches tasks to long-lived Claude agents. Each agent only sees the instructions for its current task—no context overload, no confusion about what to do next.

**The key insight:** By breaking complex work into discrete steps with clear instructions, agents can reliably tackle ambitious refactoring and codebase changes.

## Prerequisites

- A Claude instance (Claude Code, claude.ai, or API)

## Step 1: Start the Agent Pool

The agent pool is a daemon that coordinates work between your agents. In a terminal:

```bash
pnpm dlx @gsd-now/agent-pool start --pool agents
```

You can also use `npx`, `bunx`, or `yarn dlx` instead of `pnpm dlx`. Or install the package first with `pnpm add -g @gsd-now/agent-pool`.

This creates a pool named "agents". The pool manages task dispatch—when GSD submits a task, the pool routes it to an available agent.

**Keep this terminal running.** The pool stays active until you stop it.

## Step 2: Start a Claude Agent

Open Claude (Claude Code, claude.ai, or another instance) and paste these instructions:

```
You are an AI agent in a task pool. You will be given a pool name, an agent name, and an optional pool root. Your tasks are part of a larger coordinated refactor or codebase change—an orchestrator is managing the overall effort and assigning work to multiple agents.

**Follow the task instructions exactly.** They specify what work to do and what response format to use. Your response must match the format specified in the instructions—the orchestrator parses it programmatically.

Run this to see the full protocol:

pnpm dlx @gsd-now/agent-pool protocol

---

Your name is c1. The pool name is agents.
```

Claude will run the protocol command and start listening for tasks. **It will wait until GSD sends work.**

You can start multiple Claude agents with different names (c1, c2, c3) for parallel processing.

## Step 3: Run a Simple Workflow

Download a demo config:

```bash
curl -O https://raw.githubusercontent.com/gsd-now/gsd/master/crates/gsd_cli/demos/linear/config.jsonc
```

Now run it:

```bash
pnpm dlx @gsd-now/gsd run --config config.jsonc --pool agents --entrypoint-value '{}'
```

**What happens:**
1. GSD reads the config and validates the workflow
2. It submits the initial task (`Start`) to the pool
3. The pool dispatches the task to your waiting Claude agent
4. Claude follows the instructions and returns the next task(s)
5. GSD repeats until no tasks remain

Watch your Claude—it will receive tasks and respond automatically.

## Step 4: Create Your Own Refactoring Workflow

Now for something useful. Ask another Claude instance to help you create a config for refactoring a codebase:

```
I want to create a GSD workflow config that:
1. Lists all files in a folder
2. Analyzes each file for refactoring opportunities (fan-out)
3. Applies the refactors
4. Commits the changes to each file

First, run `pnpm dlx @gsd-now/gsd config schema` to see the config format.

Then look at this example for reference:
https://github.com/gsd-now/gsd/tree/master/crates/gsd_cli/demos/refactor-workflow

For more complex patterns (branching, fan-out with finally, hooks, etc.),
see the recipes: https://gsd-now.github.io/docs/recipes
```

A simple refactoring workflow might look like:

```
ListFiles → AnalyzeAndRefactor (per file) → CommitFile
```

Each step has focused instructions. The agent analyzing files doesn't need to know how to commit—it just does the refactor and passes the file to the next step.

## Example: A Simple Refactor Workflow

Here's what a basic refactor config looks like:

```json
{
  "entrypoint": "ListFiles",
  "steps": [
    {
      "name": "ListFiles",
      "value_schema": {
        "type": "object",
        "required": ["folder"],
        "properties": { "folder": { "type": "string" } }
      },
      "action": {
        "kind": "Pool",
        "instructions": "List all source files in the given folder. Return an array of AnalyzeAndRefactor tasks, one per file:\n\n```json\n[{\"kind\": \"AnalyzeAndRefactor\", \"value\": {\"file\": \"src/main.rs\"}}, ...]\n```"
      },
      "next": ["AnalyzeAndRefactor"]
    },
    {
      "name": "AnalyzeAndRefactor",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": { "file": { "type": "string" } }
      },
      "action": {
        "kind": "Pool",
        "instructions": "Read the file and identify ONE refactoring opportunity (rename a variable, extract a function, etc). Apply the refactor. Then return:\n\n```json\n[{\"kind\": \"CommitFile\", \"value\": {\"file\": \"src/main.rs\"}}]\n```\n\nIf no refactoring needed, return `[]`."
      },
      "next": ["CommitFile"]
    },
    {
      "name": "CommitFile",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": { "file": { "type": "string" } }
      },
      "action": {
        "kind": "Pool",
        "instructions": "Commit the changes to this file with a descriptive message. Return `[]` when done."
      },
      "next": []
    }
  ]
}
```

**The flow:**
- `ListFiles` scans a folder and fans out to `AnalyzeAndRefactor` tasks (one per file)
- Each `AnalyzeAndRefactor` finds and applies one refactor, then emits a `CommitFile` task
- `CommitFile` commits the changes and terminates

Save this as `refactor.jsonc` and run:

```bash
pnpm dlx @gsd-now/gsd run --config refactor.jsonc --pool agents \
  --entrypoint-value '{"folder": "./src"}'
```

For a more complete example, see the [refactor-workflow demo](https://github.com/gsd-now/gsd/tree/master/crates/gsd_cli/demos/refactor-workflow).

## Next Steps

- [Recipes](/docs/recipes) — Common patterns like fan-out, branching, and hooks
- [CLI Reference](/docs/reference/cli) — All GSD and agent_pool commands
- [Config Schema](/docs/reference/config-schema) — Full configuration options
- [Demo Configs](https://github.com/gsd-now/gsd/tree/master/crates/gsd_cli/demos) — Working examples to learn from
