# GSD (Get Sh*** Done)

GSD is a set of tools for defining task queues as type-safe state machines whose tasks are executed by long-lived agents. There are two interfaces provided: the GSD CLI and the underlying Rust libraries.

## Why?

LLMs are incredibly powerful tools. They are being asked to perform increasingly complicated, long-lived tasks. Unfortunately, the naive way to work with agents quickly hits limits. When their context becomes too full, they become forgetful and make the wrong decisions.

GSD is an attempt to provide structure and protect context, and thus enable LLMs to perform dramatically more complicated and ambitious tasks.

With GSD, you define a state machine via JSON config. Transitions between states are validated. Individual tasks are performed by long-lived agents running in a worker pool. This makes it easy to reason about the possible states and actions that your agents will be asked to perform, and the steps can be independent and smaller. The CLI provides just the needed context for an individual task, meaning that if agents are given small atomic tasks, they can more reliably perform them correctly (this has been referred to as progressive disclosure).

For example, if an agent is asked to list all the files in a folder and analyze each file, by default you would provide instructions for both tasks to the agent at the same time. With GSD, there is no need to provide both sets of instructions at once. Those instructions can be split into two steps. The agent that works on an individual task will only see exactly the instructions that it needs. With this added structure, agents can more reliably and rigorously handle tasks of increasing complexity.

See [crates/gsd_cli/demos](crates/gsd_cli/demos) for example workflows.

## Quick Start

```bash
# In one terminal, start the agent pool
pnpm dlx @gsd-now/agent-pool start --pool agents
```

In another terminal, pass this information to Claude:

```
You are an AI agent in a task pool. You will be given a pool name, an agent name, and an optional pool root. Your tasks are part of a larger coordinated refactor or codebase change—an orchestrator is managing the overall effort and assigning work to multiple agents.

**Follow the task instructions exactly.** They specify what work to do and what response format to use. Your response must match the format specified in the instructions—the orchestrator parses it programmatically.

Run this to see the full protocol:

pnpm dlx @gsd-now/agent-pool protocol

---

Your name is c1. The pool name is agents.
```

(See [crates/agent_pool/protocols/AGENT_INSTRUCTIONS.md](crates/agent_pool/protocols/AGENT_INSTRUCTIONS.md) for the full instructions.)

```bash
# In another terminal, run the GSD workflow
pnpm dlx @gsd-now/gsd run config.json --pool agents --initial '[{"kind": "Start", "value": {}}]'
```

## Components

### 1. GSD (`crates/gsd`)

A CLI tool for running a task queue defined in a configuration file, using long-lived agents operating in a worker pool.

```bash
pnpm dlx @gsd-now/gsd run config.json --pool agents --initial '[{"kind": "Start", "value": {}}]'
```

See below for detailed instructions, or [crates/gsd/DESIGN.md](crates/gsd/DESIGN.md) for the config format and protocol.

### 2. Task Queue (`crates/task_queue`)

A Rust library for defining task queues as type-safe state machines. Tasks execute arbitrary shell scripts and deserialize their stdout.

**Interfaces:**
- **Rust API** - Define tasks with compile-time type safety, state machine semantics, and automatic task chaining

See [crates/task_queue/README.md](crates/task_queue/README.md) for API documentation.

### 3. Agent Pool (`crates/agent_pool`)

A daemon that manages a pool of long-running agents. Tasks are dispatched to available agents via a file-based protocol, enabling persistent workers that don't pay startup costs per task.

```bash
# In a terminal, start the daemon
pnpm dlx @gsd-now/agent-pool start --pool agents

# From another terminal, submit a task (GSD calls this internally)
pnpm dlx @gsd-now/agent-pool submit_task --pool agents --data "task input"

# An agent calls get_task to wait for work (writes response to returned file)
pnpm dlx @gsd-now/agent-pool get_task --pool agents --name agent1
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

- [Mental Model](docs/mental-model.md) - Architecture overview and key concepts
- [Recipes](docs/recipes/README.md) - Common patterns and workflows
- [TODOs and Future Work](refactors/pending/todos.md) - Planned improvements and ideas
