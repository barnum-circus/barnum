# GSD (Get Sh*** Done)

GSD is a set of tools for defining task queues as type-safe state machines whose tasks are executed by long-lived agents. There are two interfaces provided: the GSD CLI and the underlying Rust libraries.

## What is this?

LLMs are incredibly powerful tools. They are being asked to perform increasingly complicated, long-lived tasks. Unfortunately, the naive way to work with agents quickly hits limits. When their context becomes too full, they become forgetful and make the wrong decisions.

GSD is an attempt to provide structure and enable LLMs to perform dramatically more complicated and ambitious tasks.

With GSD, you define a state machine via JSON config where individual tasks are performed by long-lived agents running in a worker pool. Transitions between states are validated. This makes it easy to reason about the possible states and actions that your agents will be asked to perform, and the steps can be independent and smaller. The CLI provides just the needed context for an individual task, meaning that if agents are given small atomic tasks, they can more reliably perform them correctly.

For example, if an agent is asked to list all the files in a folder and analyze each file, you would naively provide instructions for both tasks to the agent at the same time. With GSD, there is no need to provide both sets of instructions at once. Those instructions can be split into two steps. The agent that works on an individual task will only see exactly the instructions that it needs. With this added structure, agents can more reliably and rigorously handle tasks of increasing complexity.

See [crates/gsd_cli/demos](crates/gsd_cli/demos) for example workflows.

## Components

### 1. GSD (`crates/gsd`)

A CLI tool for running a task queue defined in a configuration file, using long-lived agents operating in a worker pool.

```bash
# Run a state machine
pnpm dlx @gsd-now/gsd run config.json --pool /tmp/pool --initial '[{"kind": "Start", "value": {}}]'

# Validate a config file
pnpm dlx @gsd-now/gsd validate config.json

# Generate documentation
pnpm dlx @gsd-now/gsd docs config.json
```

See [crates/gsd/DESIGN.md](crates/gsd/DESIGN.md) for the config format and protocol.

### 2. Task Queue (`crates/task_queue`)

A Rust library for defining task queues as type-safe state machines. Tasks execute arbitrary shell scripts and deserialize their stdout.

**Interfaces:**
- **Rust API** - Define tasks with compile-time type safety, state machine semantics, and automatic task chaining

See [crates/task_queue/README.md](crates/task_queue/README.md) for API documentation.

### 3. Agent Pool (`crates/agent_pool`)

A daemon that manages a pool of long-running agents. Tasks are dispatched to available agents via a file-based protocol, enabling persistent workers that don't pay startup costs per task.

```bash
# Start the daemon
agent_pool start /path/to/root

# Submit a task (blocks until complete)
agent_pool submit /path/to/root "task input"

# Stop the daemon
agent_pool stop /path/to/root
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

1. **Seed** - Find all `invariant.md` files in a codebase. Each describes (in English) invariants that must hold for that folder.

2. **ValidateInvariant** - An agent checks if a folder satisfies its invariants. On violation, it emits `QuickFix` tasks.

3. **QuickFix** - An agent applies a fix. When the last fix for a folder completes, re-queue `ValidateInvariant` for that folder.

4. **Retry limit** - Each `ValidateInvariant` tracks attempt count in context. After 3 failures, emit a catastrophic error instead of retrying.

```
Context {
    attempts: HashMap<PathBuf, u32>,      // folder -> attempt count
    pending_fixes: HashMap<PathBuf, u32>, // folder -> remaining fixes
    catastrophic_errors: Vec<PathBuf>,    // folders that couldn't be fixed
}
```

Setting `max_attempts = 1` turns this into a pure linter (validate only, no fixes).

## Documentation

- [Mental Model](docs/mental-model.md) - Architecture overview and key concepts
- [Recipes](docs/recipes/README.md) - Common patterns and workflows
- [TODOs and Future Work](refactors/pending/todos.md) - Planned improvements and ideas
