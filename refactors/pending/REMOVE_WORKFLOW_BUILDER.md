# Remove WorkflowBuilder

## Motivation

`workflowBuilder().workflow(() => pipeline).run()` is three layers of indirection around what should be a single function call. The builder adds no value: there's one workflow per config, no configuration steps between `workflowBuilder()` and `.workflow()`, and no reuse of the builder instance.

Replace with `runPipeline(pipeline)` or `runPipeline(pipeline, input)`. Delete `WorkflowBuilder`, `RunnableConfig`, and `WorkflowAction` (the void-input type alias is no longer needed since `runPipeline` accepts any pipeline).

## Current state

**`WorkflowBuilder`** and **`RunnableConfig`** (`libs/barnum/src/ast.ts:950-983`): Builder pattern wrapping `Config` wrapping `WorkflowAction`. Three layers for no reason.

**`WorkflowAction`** (`libs/barnum/src/ast.ts:119-123`): `Action & { __in?: void; ... }`. Constrains pipeline input to void. Used exclusively by `Config`, `config()`, `WorkflowBuilder`, and `RunnableConfig` — all being deleted.

**`Config`** (`libs/barnum/src/ast.ts:136-138`): `{ workflow: WorkflowAction<Out> }`. Used by `run()` in `run.ts` and the round-trip tests.

**`run()`** (`libs/barnum/src/run.ts`): Takes `Config`, serializes to JSON, spawns CLI with `--config <json>`.

**Demos**: All four use `workflowBuilder().workflow(() => pipeline).run()`.

**Tests**: `types.test.ts` and `round-trip.test.ts` use `workflowBuilder().workflow(() => ...)`.

## Proposed changes

### 1. Add `runPipeline` to `run.ts`

Single signature. No overloads. Takes any `Action` and an optional input.

```typescript
// libs/barnum/src/run.ts

import type { Action } from "./ast.js";
import { chain } from "./chain.js";
import { constant } from "./builtins.js";

export async function runPipeline(
  pipeline: Action,
  input?: unknown,
): Promise<void> {
  const workflow =
    input !== undefined
      ? (chain(constant(input) as any, pipeline as any) as Action)
      : pipeline;
  await run({ workflow });
}
```

When input is provided, `chain(constant(input), pipeline)` prepends a constant node. The Rust engine sees a normal Chain starting with a builtin Constant — no Rust changes needed.

### 2. Delete `WorkflowAction`, `WorkflowBuilder`, `RunnableConfig`

Remove from `libs/barnum/src/ast.ts`:
- `WorkflowAction` type alias (lines 119-123) and its JSDoc block (lines 100-117)
- `RunnableConfig` class (lines 950-969)
- `WorkflowBuilder` interface and `workflowBuilder()` function (lines 971-983)

### 3. Simplify `Config` and `config()`

```typescript
// Before
export interface Config<Out = any> {
  workflow: WorkflowAction<Out>;
}
export function config<Out>(workflow: WorkflowAction<Out>): Config<Out> {
  return { workflow };
}

// After
export interface Config {
  workflow: Action;
}
export function config(workflow: Action): Config {
  return { workflow };
}
```

No type parameters. `Config` is a serialization type — the phantom output type was never used at runtime.

### 4. Update `run()` signature

```typescript
// Before
export function run(config: Config): Promise<void>

// After — unchanged, Config just lost its type parameter
export function run(config: Config): Promise<void>
```

### 5. Update exports

```typescript
// libs/barnum/src/index.ts

// workflowBuilder disappears when deleted from ast.ts.
// Add:
export { run, runPipeline } from "./run.js";
```

### 6. Update demos

All demos use a bare top-level `runPipeline(...)` call — no `await`, no wrapper function. The process exits when the workflow completes (or errors).

**`demos/simple-workflow/run.ts`:**
```typescript
import { runPipeline, pipe } from "@barnum/barnum";
// ...handler imports...

runPipeline(
  listFiles
    .forEach(
      pipe(implementRefactor, typeCheckFiles, fixTypeErrors, commitChanges, createPullRequest),
    )
    .drop(),
);
```

**`demos/retry-on-error/run.ts`:**
```typescript
import { runPipeline, loop, tryCatch, pipe, constant, drop, withTimeout } from "@barnum/barnum";
// ...handler imports...

runPipeline(
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
import { runPipeline, pipe } from "@barnum/barnum";
// ...handler imports...

runPipeline(
  pipe(
    setup,
    listFiles.forEach(migrate({ to: "Typescript" })).drop(),
    typeCheckFix,
  ),
);
```

**`demos/identify-and-address-refactors/run.ts`:**
```typescript
import { runPipeline, pipe, constant, forEach, Option, withResource } from "@barnum/barnum";
// ...handler imports...

runPipeline(
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

### 7. Add `analyze-file` demo

Demonstrates `runPipeline(pipeline, input)` with three parallel analyses.

**`demos/analyze-file/run.ts`:**
```typescript
import { runPipeline, all } from "@barnum/barnum";
import {
  analyzeClassComponents,
  analyzeImpossibleStates,
  analyzeErrorHandling,
} from "./handlers/analyze.js";

runPipeline(
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

### 8. Update tests

**`libs/barnum/tests/types.test.ts`:**

The "rejects workflows that expect input" test is no longer relevant — `runPipeline` accepts any pipeline, with or without input. Delete it.

The "accepts workflows starting with constant" and "source handler" tests change from `workflowBuilder().workflow(() => ...)` to `config(...)`:

```typescript
it("accepts pipelines starting with constant", () => {
  const cfg = config(pipe(constant({ artifact: "test" }), verify));
  expect(cfg.workflow.kind).toBe("Chain");
});

it("source handler is accepted as pipeline", () => {
  const h = createHandler({ handle: async () => "result" }, "h");
  config(h);
});
```

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

1. **Add `runPipeline`** — Add to `run.ts`, export from `index.ts`.
2. **Delete `WorkflowAction`, `WorkflowBuilder`, `RunnableConfig`** — Remove from `ast.ts`.
3. **Simplify `Config` and `config()`** — Drop type parameters, use `Action`.
4. **Update existing demos** — Replace `workflowBuilder().workflow(() => ...).run()` with `runPipeline(...)`.
5. **Add `analyze-file` demo** — New demo with input, parallel analyses.
6. **Update tests** — Replace `workflowBuilder()` with `config()`, delete obsolete type test.

## Deferred

Rust-level `--input` CLI flag (see DEFERRED_FEATURES.md). The with-input path currently works by prepending a `constant()` node at the TypeScript level.
