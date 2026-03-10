# Introduction

Barnum is a set of tools for defining task queues as type-safe state machines whose tasks are executed by long-lived agents.

## Why Barnum?

LLMs are incredibly powerful tools. They are being asked to perform increasingly complicated, long-lived tasks. Unfortunately, the naive way to work with agents quickly hits limits. When their context becomes too full, they become forgetful and make the wrong decisions.

Barnum provides structure and protects context, enabling LLMs to perform dramatically more complicated and ambitious tasks.

### Key Features

- **Type-Safe State Machines**: Define task queues with validated state transitions
- **Progressive Disclosure**: Agents only see the instructions they need for their current task
- **Long-Lived Agents**: Workers persist across tasks, avoiding startup costs
- **JSON Configuration**: Define workflows via simple JSON config files

### Why isn't /loop sufficient?

Tools like Claude's `/loop` command are great for simple, iterative tasks. But for complex refactors and multi-step workflows, they fall short:

- **Predictability**: With Barnum, you know exactly what states your workflow can be in and what transitions are valid. You can reason about the decision tree before running it.
- **Guaranteed Structure**: The state machine enforces that agents follow the defined workflow. Invalid transitions are rejected and retried.
- **Separation of Concerns**: Each step has its own instructions, schema, and retry policy. Agents don't need to remember the entire workflow—they just handle their current task.
- **Parallelism**: Barnum naturally supports fan-out patterns where multiple tasks run concurrently, then aggregate results.
- **Auditability**: Every state transition is explicit and logged. You can trace exactly how the workflow progressed.

For simple "keep trying until it works" loops, `/loop` is fine. For complex, multi-agent workflows where you need guarantees about behavior, Barnum provides the structure that makes ambitious automation possible.

## Components

### Barnum CLI

The main command-line tool for running task queues:

```bash
barnum run --config config.jsonc --pool agents --entrypoint-value '{}'
```

### Troupe

A daemon that manages a pool of long-running agents:

```bash
troupe start --pool agents
```

### Task Queue Library

A Rust library for defining task queues as type-safe state machines with compile-time guarantees.

## Getting Started

Check out the [Quick Start guide](./quickstart) to get up and running, or browse the [recipes](./recipes/) for common workflow patterns:

- **[Linear Pipeline](./recipes/linear-pipeline.md)** — Step-by-step processing
- **[Fan-Out](./recipes/fan-out.md)** — Split one task into many parallel tasks
- **[Fan-Out with Finally](./recipes/fan-out-finally.md)** — Parallel work with aggregation on completion
- **[Sequential Processing](./recipes/sequential.md)** — Ordered, one-at-a-time execution
- **[Branching](./recipes/branching.md)** — Conditional paths based on output
- **[Branching Refactor](./recipes/branching-refactor.md)** — Route to specialized agents based on analysis
- **[Adversarial Review](./recipes/adversarial-review.md)** — Implement → judge → revise loop
- **[Error Recovery](./recipes/error-recovery.md)** — Catch failures and route to recovery steps
- **[Hooks](./recipes/hooks.md)** — Pre/post/finally hooks for data transformation and cleanup
- **[Commands](./recipes/commands.md)** — Run shell scripts instead of agents
