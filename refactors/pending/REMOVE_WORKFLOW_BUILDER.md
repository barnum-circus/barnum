# Remove WorkflowBuilder

## Motivation

`workflowBuilder().workflow(() => pipeline).run()` is three layers of indirection around what should be a single function call. `WorkflowBuilder` creates a `RunnableConfig`, which wraps a `Config`, which wraps the pipeline action. The builder adds no value: there's one workflow per config, no configuration steps between `workflowBuilder()` and `.workflow()`, and no reuse of the builder instance.

Replace with `runPipeline(pipeline)` or `runPipeline(pipeline, input)`.

## Current state

### TypeScript

**`WorkflowBuilder`** (`libs/barnum/src/ast.ts:971-983`):

```typescript
export interface WorkflowBuilder {
  workflow<Out>(build: () => WorkflowAction<Out>): RunnableConfig<Out>;
}

export function workflowBuilder(): WorkflowBuilder {
  return {
    workflow<Out>(build: () => WorkflowAction<Out>): RunnableConfig<Out> {
      return new RunnableConfig(build());
    },
  };
}
```

**`RunnableConfig`** (`libs/barnum/src/ast.ts:950-969`):

```typescript
export class RunnableConfig<Out = any> {
  readonly workflow: WorkflowAction<Out>;

  constructor(workflow: WorkflowAction<Out>) {
    this.workflow = workflow;
  }

  async run(): Promise<void> {
    const { run } = await import("./run.js");
    await run(this.toJSON());
  }

  toJSON(): Config<Out> {
    return { workflow: this.workflow };
  }
}
```

**`run()`** (`libs/barnum/src/run.ts:113`): Takes `Config`, serializes to JSON, spawns CLI with `--config <json>`.

**Demos**: All four demos use the same pattern:

```typescript
await workflowBuilder()
  .workflow(() => pipeline)
  .run();
```

**Tests**: `types.test.ts` uses `workflowBuilder().workflow(() => ...)` for type-level assertions. `round-trip.test.ts` uses it to construct serializable configs.

## Proposed changes

### 1. Add `runPipeline` to `run.ts`

Two overloads: one for void-input pipelines, one for pipelines that require input. The with-input overload prepends `constant(input)` to the pipeline internally, so no Rust-side changes are needed.

```typescript
// libs/barnum/src/run.ts

import type { Action, Pipeable, WorkflowAction } from "./ast.js";
import { chain } from "./chain.js";
import { constant } from "./builtins.js";

// Overload: pipeline with void input
export function runPipeline<TOut>(
  pipeline: WorkflowAction<TOut>,
): Promise<void>;

// Overload: pipeline with explicit input
export function runPipeline<TIn, TOut>(
  pipeline: Pipeable<TIn, TOut>,
  input: TIn,
): Promise<void>;

// Implementation
export async function runPipeline(
  pipeline: Action,
  input?: unknown,
): Promise<void> {
  const workflow =
    input !== undefined
      ? chain(constant(input) as Pipeable, pipeline as Pipeable)
      : pipeline;
  await run({ workflow: workflow as WorkflowAction });
}
```

When a pipeline has `__in?: void` (or `never` or `any`), overload 1 matches and no input is needed. When a pipeline has a concrete input type like `string`, only overload 2 matches, making `input` required.

The with-input overload constructs `Chain(Constant(input), pipeline)` at the AST level. The Rust engine sees this as a normal chain starting with a builtin constant node, so it works with the existing `Value::Null` initial advance.

### 2. Delete `WorkflowBuilder` and `RunnableConfig`

Remove from `libs/barnum/src/ast.ts`:
- `WorkflowBuilder` interface (lines 971-974)
- `workflowBuilder()` function (lines 976-983)
- `RunnableConfig` class (lines 950-969)

Keep the `config()` helper function (line 939-941) for type and round-trip tests.

### 3. Update exports

```typescript
// libs/barnum/src/index.ts

// workflowBuilder disappears from `export * from "./ast.js"` when deleted.
// Add runPipeline:
export { run, runPipeline } from "./run.js";
```

### 4. Update demos

All four existing demos change from:

```typescript
import { workflowBuilder, pipe, ... } from "@barnum/barnum";

await workflowBuilder()
  .workflow(() => pipeline)
  .run();
```

To:

```typescript
import { runPipeline, pipe, ... } from "@barnum/barnum";

await runPipeline(pipeline);
```

**`demos/simple-workflow/run.ts`:**
```typescript
await runPipeline(
  listFiles
    .forEach(
      pipe(implementRefactor, typeCheckFiles, fixTypeErrors, commitChanges, createPullRequest),
    )
    .drop(),
);
```

**`demos/retry-on-error/run.ts`:**
```typescript
await runPipeline(
  loop((recur, done) =>
    tryCatch(
      (throwError) =>
        pipe(
          stepA.mapErr(drop).unwrapOr(done).drop(),
          withTimeout(constant(2_000), stepB.unwrapOr(throwError))
            .mapErr(constant("stepB: timed out"))
            .unwrapOr(throwError)
            .drop(),
          stepC.unwrapOr(throwError).drop(),
        ),
      logError.then(recur),
    ),
  ),
);
```

**`demos/convert-folder-to-ts/run.ts`:**
```typescript
await runPipeline(
  pipe(
    setup,
    listFiles.forEach(migrate({ to: "Typescript" })).drop(),
    typeCheckFix,
  ),
);
```

**`demos/identify-and-address-refactors/run.ts`:**
```typescript
await runPipeline(
  pipe(
    constant({ folder: srcDir }),
    listTargetFiles,
    forEach(analyze).flatten(),
    forEach(assessWorthiness).then(Option.collect()),
    forEach(
      withResource({
        create: createBranchWorktree,
        action: implementAndReview,
        dispose: deleteWorktree,
      }),
    ),
  ),
);
```

### 5. Add `analyze-file` demo

A new demo that takes input via `runPipeline(pipeline, input)`. Three analyses run in parallel on a file path.

**`demos/analyze-file/run.ts`:**
```typescript
/**
 * Analyze-file demo: run three independent analyses on a single file.
 *
 * Demonstrates: runPipeline with input, all (parallel execution).
 *
 * Usage: pnpm exec tsx run.ts
 */

import { runPipeline, all } from "@barnum/barnum";
import {
  analyzeClassComponents,
  analyzeImpossibleStates,
  analyzeErrorHandling,
} from "./handlers/analyze.js";

await runPipeline(
  all(analyzeClassComponents, analyzeImpossibleStates, analyzeErrorHandling),
  "source/index.ts",
);
```

**`demos/analyze-file/handlers/analyze.ts`:**
```typescript
import { createHandler } from "@barnum/barnum";
import { z } from "zod";

export const analyzeClassComponents = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: file }) => {
      console.error(`[analyzeClassComponents] Scanning ${file} for class components...`);
      return `${file}: no class components found`;
    },
  },
  "analyzeClassComponents",
);

export const analyzeImpossibleStates = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: file }) => {
      console.error(`[analyzeImpossibleStates] Scanning ${file} for impossible states...`);
      return `${file}: 2 impossible states found`;
    },
  },
  "analyzeImpossibleStates",
);

export const analyzeErrorHandling = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: file }) => {
      console.error(`[analyzeErrorHandling] Scanning ${file} for error handling issues...`);
      return `${file}: 1 unhandled error path`;
    },
  },
  "analyzeErrorHandling",
);
```

**`demos/analyze-file/package.json`:**
```json
{
  "name": "analyze-file-demo",
  "version": "0.0.1",
  "private": true,
  "type": "module",
  "scripts": {
    "typecheck": "tsc --noEmit",
    "demo": "if [ -f ../../Cargo.toml ]; then pnpm -C ../../libs/barnum run build; fi && tsx run.ts"
  },
  "dependencies": {
    "@barnum/barnum": "link:../../libs/barnum",
    "@types/node": "^22.0.0",
    "tsx": "^4.19.0",
    "typescript": "^5.7.0",
    "zod": "^4.3.6"
  }
}
```

**`demos/analyze-file/tsconfig.json`:**
```json
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "ES2022",
    "moduleResolution": "bundler",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "noEmit": true
  },
  "include": ["run.ts", "handlers"]
}
```

### 6. Update tests

**`libs/barnum/tests/types.test.ts`** — "config entry point" tests:

```typescript
// Before
it("rejects workflows that expect input", () => {
  // @ts-expect-error
  workflowBuilder().workflow(() => verify);
});

it("accepts workflows starting with constant", () => {
  const cfg = workflowBuilder().workflow(() =>
    pipe(constant({ artifact: "test" }), verify),
  );
  expect(cfg.workflow.kind).toBe("Chain");
});

// After
it("rejects pipelines that expect input", () => {
  // @ts-expect-error — verify expects { artifact: string } input
  config(verify);
});

it("accepts pipelines starting with constant", () => {
  const cfg = config(pipe(constant({ artifact: "test" }), verify));
  expect(cfg.workflow.kind).toBe("Chain");
});
```

The "source handler" tests similarly change from `workflowBuilder().workflow(() => h)` to `config(h)`.

**`libs/barnum/tests/round-trip.test.ts`** — replace all `workflowBuilder().workflow(() => ...)` with `config(...)`:

```typescript
// Before
const cfg = workflowBuilder().workflow(() =>
  pipe(constant({ project: "test" }), setup),
);

// After
const cfg = config(pipe(constant({ project: "test" }), setup));
```

## Task list

1. **Add `runPipeline` with both overloads** — Add to `run.ts`, export from `index.ts`.
2. **Delete `WorkflowBuilder` and `RunnableConfig`** — Remove from `ast.ts`.
3. **Update existing demos** — Replace `workflowBuilder().workflow(() => ...).run()` with `runPipeline(...)` in all four demos.
4. **Add `analyze-file` demo** — New demo with input, parallel analyses.
5. **Update tests** — Replace `workflowBuilder()` usage in `types.test.ts` and `round-trip.test.ts` with `config()`.

## Deferred

Rust-level `--input` CLI flag (see DEFERRED_FEATURES.md). The with-input overload currently works by prepending a `constant()` node at the TypeScript level.
