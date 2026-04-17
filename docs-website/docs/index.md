---
image: /img/og/index.png
---

# Introduction

Barnum is a programming language for asynchronous programming that is geared towards making it easy to precisely orchestrate agents.

## Why?

LLMs are incredibly powerful tools. They are being asked to perform increasingly complicated, long-lived tasks. Unfortunately, the naive way to work with agents quickly hits limits. When their context becomes too full, they become forgetful and make the wrong decisions. You can't rely on them to faithfully execute a complicated, multi-step plan.

### 🦁 A choreographed show

Barnum workflows are state machines. Transitions are declared up front, steps are independent and small, and the possible states are easy to reason about. No hoping the agent stays on track.

### 🐘 The right performer for each act

Each agentic step receives only the context it needs. If an agent is asked to both analyze a file for refactoring opportunities *and* implement the refactors, it has to hold both tasks in context at once. With Barnum, analysis and implementation are separate steps. The implementing agent only sees the refactor description — not the analysis instructions.

### 🐯 No one goes off script

Each handler executes in its own isolated subprocess. No handler sees another handler's context. The agent performing the refactor has no idea that a type-check step follows — it just receives a filename and a prompt.

## A simple example

Handlers are the building blocks of a Barnum workflow. Today, handlers are either built-in primitives or exported TypeScript async functions. (Support for other languages is planned.)

```ts
// handlers/steps.ts
import { createHandler } from "@barnum/barnum/runtime";
import { z } from "zod";

export const listFiles = createHandler({
  outputValidator: z.array(z.string()),
  handle: async () => {
    return readdirSync("src/").filter(f => f.endsWith(".ts"));
  },
}, "listFiles");

export const refactor = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    await callAgent({
      prompt: `Refactor ${file} to replace all class-based React components with functional components using hooks.`,
      allowedTools: ["Read", "Edit"],
    });
  },
}, "refactor");

// ... typeCheck, fix, commit, createPR
```

You compose handlers into a workflow using postfix methods like `.then()` (sequential) and `.forEach()` (fan-out):

```ts
// run.ts
import { runPipeline } from "@barnum/barnum/pipeline";
import { listFiles, refactor, typeCheck, fix, commit, createPR } from "./handlers/steps.js";

runPipeline(
  listFiles.forEach(
    refactor.then(typeCheck).then(fix).then(commit).then(createPR)
  ),
);
```

`listFiles` runs once and returns an array of filenames. `.forEach()` fans out — each filename flows through `refactor → typeCheck → fix → commit → createPR`, with each file processed in parallel.

Each handler executes in its own isolated Node.js subprocess. The Rust runtime manages the state machine: it tracks which handlers are pending, dispatches them, collects results, and advances the workflow. No handler sees another handler's context. The agent performing the refactor has no idea that a type-check step follows — it just receives a filename and a prompt.

## Learn more

- [Quickstart](./quickstart) — install, write handlers, compose a workflow, run it
- [Patterns](./patterns/) — the building blocks: [parallel execution](./patterns/parallel-execution), [branching](./patterns/branching), [looping](./patterns/looping), [error handling](./patterns/error-handling), [timeout](./patterns/timeout), [racing](./patterns/racing), [context and variables](./patterns/context-and-variables), [early return](./patterns/early-return)
- [Repertoire](./repertoire/) — real-world workflows: [adversarial review](./repertoire/adversarial-review), [code review](./repertoire/code-review), [codebase migration](./repertoire/codebase-migration), [incident triage](./repertoire/incident-triage), and [more](./repertoire/)
- [Builtins reference](./reference/builtins) — every combinator with its TypeScript type signature and postfix availability
- [CLI reference](./reference/cli) — how to run workflows, binary resolution, `callClaude()`
- [Architecture](./architecture/) — the TypeScript AST, Rust compiler, algebraic effect handlers, and validation system

## Demos

Browse the [demos](https://github.com/barnum-circus/barnum/tree/master/demos) for complete working examples:

| Demo | Description |
|---|---|
| [`simple-workflow`](https://github.com/barnum-circus/barnum/tree/master/demos/simple-workflow) | List files, then refactor/typecheck/fix/commit/PR each one in parallel |
| [`retry-on-error`](https://github.com/barnum-circus/barnum/tree/master/demos/retry-on-error) | Fallible pipeline with `tryCatch`, `withTimeout`, and `loop` for retry |
| [`convert-folder-to-ts`](https://github.com/barnum-circus/barnum/tree/master/demos/convert-folder-to-ts) | Convert JS files to TypeScript with an LLM, iterating on type errors |
| [`identify-and-address-refactors`](https://github.com/barnum-circus/barnum/tree/master/demos/identify-and-address-refactors) | Discover refactoring opportunities, implement them in worktrees, review with an LLM |
