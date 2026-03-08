# Introduction

GSD (Get Sh*** Done) is a set of tools for defining task queues as type-safe state machines whose tasks are executed by long-lived agents.

## Why GSD?

LLMs are incredibly powerful tools. They are being asked to perform increasingly complicated, long-lived tasks. Unfortunately, the naive way to work with agents quickly hits limits. When their context becomes too full, they become forgetful and make the wrong decisions.

GSD provides structure and protects context, enabling LLMs to perform dramatically more complicated and ambitious tasks.

### Key Features

- **Type-Safe State Machines**: Define task queues with validated state transitions
- **Progressive Disclosure**: Agents only see the instructions they need for their current task
- **Long-Lived Agents**: Workers persist across tasks, avoiding startup costs
- **JSON Configuration**: Define workflows via simple JSON config files

## Components

### GSD CLI

The main command-line tool for running task queues:

```bash
pnpm dlx @gsd-now/gsd@main run --config config.jsonc --pool agents --initial-state '[{"kind": "Start", "value": {}}]'
```

### Agent Pool

A daemon that manages a pool of long-running agents:

```bash
pnpm dlx @gsd-now/agent-pool@main start --pool agents
```

### Task Queue Library

A Rust library for defining task queues as type-safe state machines with compile-time guarantees.

## Getting Started

Check out the [Quick Start guide](./quickstart) to get up and running.
