# Barnum

Barnum is a set of tools for defining task queues as type-safe state machines whose tasks are executed by long-lived agents. There are two interfaces provided: the Barnum CLI and the underlying Rust libraries.

> **Note:** Examples use `pnpm dlx`. You can also use `npx` or install globally with `pnpm add -g @barnum/barnum @barnum/troupe`.

## Why?

LLMs are incredibly powerful tools. They are being asked to perform increasingly complicated, long-lived tasks. Unfortunately, the naive way to work with agents quickly hits limits. When their context becomes too full, they become forgetful and make the wrong decisions.

Barnum is an attempt to provide structure and protect context, and thus enable LLMs to perform dramatically more complicated and ambitious tasks.

With Barnum, you define a state machine via JSON config. Transitions between states are validated. This makes it easy to reason about the possible states and actions that your agents will be asked to perform, and the steps can be independent and smaller. The CLI provides just the needed context for an individual task, meaning that if agents are given small atomic tasks, they can more reliably perform them correctly (this has been referred to as progressive disclosure).

For example, if an agent is asked to list all the files in a folder and analyze each file, by default you would provide instructions for both tasks to the agent at the same time. With Barnum, there is no need to provide both sets of instructions at once. Those instructions can be split into two steps. The agent that works on an individual task will only see exactly the instructions that it needs. With this added structure, agents can more reliably and rigorously handle tasks of increasing complexity.

See [crates/barnum_cli/demos](crates/barnum_cli/demos) for example workflows.

### Why isn't /loop sufficient?

Tools like Claude's `/loop` command are great for simple, iterative tasks. But for complex refactors and multi-step workflows, they fall short:

- **Predictability**: With Barnum, you know exactly what states your workflow can be in and what transitions are valid. You can reason about the decision tree before running it.
- **Guaranteed Structure**: The state machine enforces that agents follow the defined workflow. Invalid transitions are rejected and retried.
- **Separation of Concerns**: Each step has its own instructions, schema, and retry policy. Agents don't need to remember the entire workflow—they just handle their current task.
- **Parallelism**: Barnum naturally supports fan-out patterns where multiple tasks run concurrently, then aggregate results.
- **Auditability**: Every state transition is explicit and logged. You can trace exactly how the workflow progressed.

For simple "keep trying until it works" loops, `/loop` is fine. For complex, multi-agent workflows where you need guarantees about behavior, Barnum provides the structure that makes ambitious automation possible.

## Quick Start

```bash
# In one terminal, start the agent pool
pnpm dlx @barnum/troupe start --pool agents
```

Pass this information to a Claude instance:

```
You are an AI agent in a task pool. You will be given a pool name, an agent name, and an optional pool root. Your tasks are part of a larger coordinated refactor or codebase change—an orchestrator is managing the overall effort and assigning work to multiple agents.

**Follow the task instructions exactly.** They specify what work to do and what response format to use. Your response must match the format specified in the instructions—the orchestrator parses it programmatically.

Run this to see the full protocol:

pnpm dlx @barnum/troupe protocol

---

Your name is c1. The pool name is agents.
```

You can pass this to multiple Claudes; make sure they have distinct names for debugging purposes. See [AGENT_INSTRUCTIONS.md](crates/troupe/protocols/AGENT_INSTRUCTIONS.md) and [AGENT_PROTOCOL.md](crates/troupe/protocols/AGENT_PROTOCOL.md) for full details.

If you don't have a config, download a demo:

```bash
curl -O https://raw.githubusercontent.com/barnum-circus/barnum/master/crates/barnum_cli/demos/linear/config.jsonc
```

Then run the Barnum workflow:

```bash
pnpm dlx @barnum/barnum run config.jsonc --pool agents
```

## Creating Config Files

To see the JSON schema for config files:

```bash
pnpm dlx @barnum/barnum config schema
```

**Tip for AI agents:** When asking an AI to create a Barnum config, tell it to run `barnum config schema` first to see all available fields and their types.

## Components

### 1. Barnum (`crates/barnum`)

A CLI tool for running a task queue defined in a configuration file, using long-lived agents operating in a worker pool.

```bash
pnpm dlx @barnum/barnum run config.jsonc --pool agents
```

See below for detailed instructions, or [crates/barnum/DESIGN.md](crates/barnum/DESIGN.md) for the config format and protocol.

### 2. Task Queue (`crates/task_queue`)

A Rust library for defining task queues as type-safe state machines. Tasks execute arbitrary shell scripts and deserialize their stdout.

**Interfaces:**
- **Rust API** - Define tasks with compile-time type safety, state machine semantics, and automatic task chaining

See [crates/task_queue/README.md](crates/task_queue/README.md) for API documentation.

### 3. Troupe (`crates/troupe`)

A daemon that manages a pool of long-running agents. Tasks are dispatched to available agents via a file-based protocol, enabling persistent workers that don't pay startup costs per task.

```bash
# In a terminal, start the daemon
pnpm dlx @barnum/troupe start --pool agents

# From another terminal, submit a task (Barnum calls this internally)
pnpm dlx @barnum/troupe submit_task --pool agents --data "task input"

# An agent calls get_task to wait for work (writes response to returned file)
pnpm dlx @barnum/troupe get_task --pool agents --name agent1
# Returns JSON with response_file path - agent writes response there, then calls get_task again
```

## Example Use Cases

### Code Analysis and Refactoring Pipeline

A queue with two task types that form a pipeline:

1. **AnalyzeFile** - An agent analyzes a source file, identifying potential refactors
2. **PerformRefactor** - An agent executes a specific refactor

The workflow:
- Seed the queue with `AnalyzeFile` tasks for each source file
- Analysis agents process files and emit `PerformRefactor` tasks back to the queue
- Refactor agents pick up those tasks and apply changes
- The queue drains when all analysis is complete and all refactors are applied

### Invariant Enforcement

A self-healing linter that finds and fixes violations:

1. **FindInvariants** - Find all `invariant.md` files in a codebase. Each describes (in English) invariants that must hold for that folder.
2. **CreateValidateInvariantTasks** - Create a task for each file within a folder for a given invariant.
3. **ValidateInvariantForFile** - An agent checks if a file satisfies its invariants. On violation, it emits `QuickFix` tasks.
4. **QuickFix** - An agent applies a fix.

## Documentation

- [Recipes](docs/recipes/README.md) - Common patterns and workflows
- [TODOs and Future Work](refactors/pending/todos.md) - Planned improvements and ideas
