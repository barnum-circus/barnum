---
image: /img/og/quickstart.png
---

# Quickstart

This guide walks you through running your first Barnum workflow. We use Claude in this example, but Barnum works with any AI agent that can follow instructions and write to a file.

## Prerequisites

- An AI agent (Claude Code, claude.ai, ChatGPT, Cursor, or any agent that can run shell commands)

## Step 1: Start the Troupe

The troupe is a daemon that coordinates work between your agents. In a terminal:

```bash
pnpm dlx @barnum/troupe start
```

You can also use `npx`, `bunx`, or `yarn dlx` instead of `pnpm dlx`. Or install the package first with `pnpm add -g @barnum/troupe`.

This starts the default pool. The troupe manages task dispatch, routing each submitted task to an available agent.

**Keep this terminal running.** The pool stays active until you stop it.

:::info[tmux launcher script]
Want a one-command setup? Download [`barnum-dev.sh`](https://raw.githubusercontent.com/barnum-circus/barnum/master/docs-website/static/barnum-dev.sh) that creates a tmux session with the troupe pool, an orchestrator, and configurable agent windows. Currently set up for Claude Code. Customize it for your setup.

```bash
curl -O https://raw.githubusercontent.com/barnum-circus/barnum/master/docs-website/static/barnum-dev.sh
chmod +x barnum-dev.sh
./barnum-dev.sh 3  # start with 3 agents
```
:::

## Step 2: Instruct Your Agent to Join the Troupe

Open your agent (Claude Code, ChatGPT, Cursor, etc.) and paste these instructions:

````
# Agent Instructions

You are an AI agent in a task pool. You will be given an **agent name**. **Pool
name** and **root** are optional — do not guess them if not provided.

## IMPORTANT: Get the full protocol first

**Before doing anything else**, run this command to get the complete protocol
documentation:

```bash
pnpm dlx @barnum/troupe protocol --name <YOUR_NAME> [--pool <POOL_NAME>] [--root <ROOT>]
```

This will give you the exact JSON formats, response requirements, and the agent loop
structure. **Do not proceed without reading the protocol.**

## Quick Summary

1. You are a **long-lived worker** - keep looping until shutdown
2. Call `get_task` to receive work (blocks until task available)
3. Do everything in the task message
4. Write your JSON response to the `response_file` path
5. Immediately call `get_task` again for the next task
6. If you receive a **Kicked** message, kill the `get_task` process and exit

Your name is c1.
````

Your agent will run the protocol command and start listening for tasks. **It will wait until Barnum sends work.**

You can start multiple agents with different names (c1, c2, c3) for parallel processing.

## Step 3: Showtime

Download a demo config:

```bash
curl -O https://raw.githubusercontent.com/barnum-circus/barnum/master/crates/barnum_cli/demos/linear/config.jsonc
```

Now run it:

```bash
pnpm dlx @barnum/barnum run --config config.jsonc
```

**What happens:**
1. Barnum reads the config and validates the workflow
2. It dispatches the opening act (`Start`) to the troupe
3. The pool dispatches the task to your waiting agent
4. The agent follows the instructions and returns the next task(s)
5. Barnum repeats until no tasks remain

Watch your agent—it will receive tasks and respond automatically.

## Step 4: Write Your Own Programme

Now for something useful. Ask your agent to help you create a config for refactoring a codebase:

```
I want to create a Barnum workflow config that:
1. Lists all files in a folder
2. Analyzes each file for refactoring opportunities (fan-out)
3. Applies the refactors
4. Commits the changes to each file

First, run `pnpm dlx @barnum/barnum config schema` to get the Zod TypeScript schema describing the config format.

Then look at this example for reference:
https://github.com/barnum-circus/barnum/tree/master/crates/barnum_cli/demos/refactor-workflow

For more complex patterns (branching, fan-out with finally, hooks, etc.),
see the repertoire: https://barnum-circus.github.io/docs/repertoire
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
pnpm dlx @barnum/barnum run --config refactor.jsonc \
  --entrypoint-value '{"folder": "./src"}'
```

For a more complete example, see the [refactor-workflow demo](https://github.com/barnum-circus/barnum/tree/master/crates/barnum_cli/demos/refactor-workflow).

## Next Steps

- [Repertoire](/docs/repertoire): common routines like fan-out, branching, and hooks
- [CLI Reference](/docs/reference/cli): all Barnum and troupe commands
- [Config Schema](/docs/reference/config-schema): full configuration options
- [Demo Configs](https://github.com/barnum-circus/barnum/tree/master/crates/barnum_cli/demos): working examples to learn from
