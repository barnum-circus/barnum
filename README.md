# Barnum

Barnum is a programming language for asynchronous programming that is geared towards making it easy to precisely orchestrate agents.

## Why?

LLMs are incredibly powerful tools. They are being asked to perform increasingly complicated, long-lived tasks. Unfortunately, the naive way to work with agents quickly hits limits. When their context becomes too full, they become forgetful and make the wrong decisions.

Barnum is an attempt to enable LLMs to perform dramatically more complicated, ambitious tasks. With Barnum, you define an asynchronous workflow, which is effectively a state machine. This makes it easy to reason about the possible states and actions that your agents will be asked to perform, and the steps can be independent and small.

Each step in a workflow receives only the context it needs. If an agent is asked to list files in a folder and then analyze each file, the analyzing agent only sees the instructions for analysis — not the listing step. This progressive disclosure of context means agents can more reliably handle tasks of increasing complexity.

## How it works

You write workflows in TypeScript using Barnum's combinator library. Handlers are async functions that do the actual work (call an LLM, run a shell command, transform data). Combinators like `pipe`, `forEach`, `loop`, and `branch` compose handlers into workflows.

The TypeScript DSL compiles to a serializable AST. A Rust engine executes the workflow, managing the state machine, dispatching handlers, and enforcing structure. Input and output schemas (defined via Zod) are validated at runtime at every handler boundary.

### Example: simple workflow

```ts
// handlers/steps.ts
import { createHandler } from "@barnum/barnum";
import { z } from "zod";

export const listFiles = createHandler({
  outputValidator: z.array(z.string()),
  handle: async () => ["auth.ts", "database.ts", "routes.ts"],
}, "listFiles");

export const refactor = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    await callClaude({ prompt: `Refactor ${file}` });
  },
}, "refactor");
```

```ts
// run.ts
import { workflowBuilder, pipe } from "@barnum/barnum";
import { listFiles, refactor, typeCheck, fix, commit, createPR } from "./handlers/steps.js";

await workflowBuilder()
  .workflow(() =>
    listFiles
      .forEach(pipe(refactor, typeCheck, fix, commit, createPR))
      .drop()
  )
  .run();
```

`listFiles` runs once, returns an array of filenames. `forEach` fans out — each filename flows through the pipeline of `refactor → typeCheck → fix → commit → createPR` in parallel.

### Combinators

| Combinator | What it does |
|---|---|
| `pipe(a, b, c)` | Sequential composition — output of `a` feeds into `b`, then `c` |
| `handler.forEach(action)` | Fan out — runs `action` once per element of the array |
| `loop((recur, done) => ...)` | Repeat until `done` is called |
| `.branch({ A: ..., B: ... })` | Discriminated union dispatch — route by `kind` field |
| `tryCatch((throw) => body, catch)` | Error handling with typed error channel |
| `withTimeout(duration, action)` | Race an action against a timer |
| `all(a, b, c)` | Run actions in parallel, collect results as a tuple |
| `bindInput(params => ...)` | Capture the input value for use later in the pipeline |
| `augment(action)` | Run a side computation and merge the result into the input |

### Handlers

Handlers are created with `createHandler`. They can declare Zod validators for their input and output:

```ts
export const analyze = createHandler({
  inputValidator: z.object({ file: z.string() }),
  outputValidator: z.array(z.object({
    description: z.string(),
    scope: z.enum(["function", "module", "cross-file"]),
  })),
  handle: async ({ value }) => {
    // value is typed as { file: string }
    return await callClaude({ prompt: `Analyze ${value.file}` });
  },
}, "analyze");
```

Schemas are compiled into JSON Schema validators at workflow init. If a handler receives input or produces output that violates its schema, the workflow terminates with a validation error.

## Demos

| Demo | Description |
|---|---|
| [`simple-workflow`](demos/simple-workflow) | List files, then refactor/typecheck/fix/commit/PR each one in parallel |
| [`retry-on-error`](demos/retry-on-error) | Fallible pipeline with `tryCatch`, `withTimeout`, and `loop` for retry |
| [`convert-folder-to-ts`](demos/convert-folder-to-ts) | Convert JS files to TypeScript with Claude, iterating on type errors |
| [`identify-and-address-refactors`](demos/identify-and-address-refactors) | Discover refactoring opportunities, implement them in worktrees, review with Claude |

Run a demo:

```bash
cd demos/simple-workflow
pnpm install
pnpm run demo
```

## Architecture

```
TypeScript DSL (libs/barnum)
  → Serializable AST (JSON)
    → Rust engine (crates/barnum_engine)
      → Event loop + scheduler (crates/barnum_event_loop)
        → Handler subprocess execution (crates/barnum_typescript_handler)
```

The TypeScript library defines the workflow. `workflowBuilder().run()` serializes the AST to JSON and spawns the Rust binary, which flattens the AST into a `FlatConfig`, manages frames and task dispatch, and executes handlers as subprocesses.
