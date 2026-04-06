# Barnum

Barnum is a programming language for asynchronous programming that is geared towards making it easy to precisely orchestrate agents.

## Why?

LLMs are incredibly powerful tools. They are being asked to perform increasingly complicated, long-lived tasks. Unfortunately, the naive way to work with agents quickly hits limits. When their context becomes too full, they become forgetful and make the wrong decisions. You can't rely on them to faithfully execute a complicated, multi-step plan.

Barnum is an attempt to enable LLMs to perform dramatically more complicated, ambitious tasks. With Barnum, you define an asynchronous workflow, which is effectively a state machine. This makes it easy to reason about the possible states and actions that your agents will be asked to perform, and the steps can be independent and small.

With Barnum, it's easy to have each agentic step receive only the context it needs. If an agent is asked to both analyze a file for refactoring opportunities *and* implement the refactors, you're forcing it to hold both tasks in context at once. With Barnum, analysis and implementation are separate steps. The implementing agent only sees the refactor description — not the analysis instructions. This progressive disclosure of context means agents can more reliably handle tasks of increasing complexity.

## A simple example

Handlers are the building blocks of a Barnum workflow. Today, handlers are either built-in primitives or exported TypeScript async functions. (Support for other languages is planned.)

```ts
// handlers/steps.ts
import { createHandler } from "@barnum/barnum";
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

You compose handlers into a workflow using combinators like `pipe` (sequential) and `forEach` (fan-out):

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

> See the full working version: [`demos/simple-workflow`](https://github.com/barnum-circus/barnum/tree/master/demos/simple-workflow)

`listFiles` runs once and returns an array of filenames. `forEach` fans out — each filename flows through `refactor → typeCheck → fix → commit → createPR`, with each file processed in parallel.

Each handler executes in its own isolated Node.js subprocess. The Rust runtime manages the state machine: it tracks which handlers are pending, dispatches them, collects results, and advances the workflow. No handler sees another handler's context. The agent performing the refactor has no idea that a type-check step follows — it just receives a filename and a prompt.

## Why not just write this in JavaScript?

This example is simple. You could probably ask your favorite LLM to one-shot the orchestration script, and it would do a decent job.

When the workflow grows in complexity, you might reach for plan mode or write a markdown file describing the steps. That works for a while. But what happens when the plan has 40 steps across 15 files with conditional branches, retries on failure, parallel fan-out, and a review loop? Good luck getting an agent to faithfully and reliably execute that plan.

And in practice, you *do* want the complicated version. You don't want `listFiles` done by an agent — it's deterministic, just read the filesystem. You don't want a single `refactor` step — you want the agent to refactor, then evaluate the result, then type-check, then fix errors in a loop until it's clean:

```ts
const refactorWithRetry = pipe(
  refactor,
  evaluate,
  loop((recur) =>
    pipe(typeCheck, classifyErrors).branch({
      HasErrors: pipe(forEach(fix).drop(), recur),
      Clean: drop,
    })
  ),
  commit,
  createPR,
);

await workflowBuilder()
  .workflow(() => listFiles.forEach(refactorWithRetry).drop())
  .run();
```

Now type errors are fixed in a loop — the agent keeps fixing until the code is clean. And this is still a simplified version. A real workflow might add review steps, worktree isolation, retry-on-timeout, or error escalation.

The problem isn't that any individual piece is hard. The problem is that expressing a precise, complicated asynchronous workflow in prose or ad-hoc scripts is fragile. A programming language geared towards orchestration is what you actually want — one where `loop`, `branch`, `tryCatch`, `forEach`, and `pipe` are first-class constructs with type-safe composition.

## Demos

| Demo | Description |
|---|---|
| [`simple-workflow`](demos/simple-workflow) | List files, then refactor/typecheck/fix/commit/PR each one in parallel |
| [`retry-on-error`](demos/retry-on-error) | Fallible pipeline with `tryCatch`, `withTimeout`, and `loop` for retry |
| [`convert-folder-to-ts`](demos/convert-folder-to-ts) | Convert JS files to TypeScript with an LLM, iterating on type errors |
| [`identify-and-address-refactors`](demos/identify-and-address-refactors) | Discover refactoring opportunities, implement them in worktrees, review with an LLM |

```bash
cd demos/simple-workflow
pnpm install
pnpm run demo
```

## Repertoire

The [Repertoire](https://barnum-circus.github.io/docs/repertoire/) showcases advanced patterns — `tryCatch` with retry, `withTimeout`, worktree isolation, LLM-powered code review loops, and more.

## How Barnum works

- [Architecture overview](https://barnum-circus.github.io/docs/architecture/)
- [How the compiler works](https://barnum-circus.github.io/docs/architecture/compiler)
- [Algebraic effect handlers](https://barnum-circus.github.io/docs/architecture/algebraic-effect-handlers)
