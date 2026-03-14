# Introduction

Barnum is a ringmaster for AI agents. It orchestrates complex multi-step workflows while keeping each agent focused on exactly one task at a time.

## Why Barnum?

LLMs are incredibly powerful tools. They are being asked to perform increasingly complicated, long-lived tasks. Unfortunately, the naive way to work with agents quickly hits limits. When their context becomes too full, they become forgetful and make the wrong decisions.

Barnum provides structure and protects context, enabling LLMs to perform dramatically more complicated and ambitious tasks.

### Key Features

- **Type-Safe State Machines**: Define task queues with validated state transitions
- **Progressive Disclosure**: Agents only see the instructions they need for their current task
- **Long-Lived Agents**: Workers persist across tasks, avoiding startup costs
- **JSON Configuration**: Define workflows via simple JSON config files

### Why isn't /loop sufficient?

Tools like Claude's `/loop` command (and similar features in other agents) are great for simple, iterative tasks. But for complex refactors and multi-step workflows, they fall short:

- **Predictability**: With Barnum, you know exactly what states your workflow can be in and what transitions are valid. You can reason about the decision tree before running it.
- **Guaranteed Structure**: The state machine enforces that agents follow the defined workflow. Invalid transitions are rejected and retried.
- **Separation of Concerns**: Each step has its own instructions, schema, and retry policy. Agents don't need to remember the entire workflow. They just handle their current task.
- **Parallelism**: Barnum naturally supports fan-out patterns where multiple tasks run concurrently, then aggregate results.
- **Auditability**: Every state transition is explicit and logged. You can trace exactly how the workflow progressed.

For simple "keep trying until it works" loops, `/loop` is fine. For complex, multi-agent workflows where you need guarantees about behavior, Barnum provides the structure that makes ambitious automation possible.

## Components

:::note
These examples use `pnpm dlx` to run Barnum without installing it. You can use any package runner (`npx`, `yarn dlx`, etc.) or install it directly with `pnpm add -g @barnum/barnum`.
:::

### Barnum CLI

The main command-line tool for running task queues:

```bash
pnpm dlx @barnum/barnum run --config config.jsonc
```

### Troupe

A daemon that manages a pool of long-running agents:

```bash
pnpm dlx @barnum/barnum troupe start
```

### Task Queue Library

A Rust library for defining task queues as type-safe state machines with compile-time guarantees.

## Getting Started

Check out the [Quickstart guide](./quickstart) to get up and running, or browse the [repertoire](./repertoire/) for common routines:

- **[Linear Pipeline](./repertoire/linear-pipeline.md)**: step-by-step processing
- **[Fan-Out](./repertoire/fan-out.md)**: split one task into many parallel tasks
- **[Fan-Out with Finally](./repertoire/fan-out-finally.md)**: parallel work with aggregation on completion
- **[Sequential Processing](./repertoire/sequential.md)**: ordered, one-at-a-time execution
- **[Branching](./repertoire/branching.md)**: conditional paths based on output
- **[Branching Refactor](./repertoire/branching-refactor.md)**: route to specialized agents based on analysis
- **[Adversarial Review](./repertoire/adversarial-review.md)**: implement, judge, revise loop
- **[Error Recovery](./repertoire/error-recovery.md)**: catch failures and route to recovery steps
- **[Hooks](./repertoire/hooks.md)**: pre/post/finally hooks for data transformation and cleanup
- **[Commands](./repertoire/commands.md)**: run shell scripts instead of agents
- **[Code Review](./repertoire/code-review.md)**: parallel PR review with standards and security checks
- **[Legal Review](./repertoire/legal-review.md)**: parallel contract analysis with final recommendation
- **[Validation](./repertoire/validation.md)**: schema validation for inputs and outputs
