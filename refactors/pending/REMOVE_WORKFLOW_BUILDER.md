# Remove WorkflowBuilder

## Motivation

`workflowBuilder().workflow(() => pipeline).run()` is three layers of indirection around what should be a single function call. `WorkflowBuilder` creates a `RunnableConfig`, which wraps a `Config`, which wraps the pipeline action. The builder adds no value: there's one workflow per config, no configuration steps between `workflowBuilder()` and `.workflow()`, and no reuse of the builder instance.

Replace with `runPipeline(pipeline)`. Pipelines that need input use `constant()` at the head of the pipeline, as all current demos already do.

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

`runPipeline` takes a `WorkflowAction` (pipeline with void input) and runs it. Input is always provided via `constant()` at the pipeline head.

```typescript
// libs/barnum/src/run.ts

export async function runPipeline<TOut>(
  pipeline: WorkflowAction<TOut>,
): Promise<void> {
  await run({ workflow: pipeline });
}
```

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

All four demos change from:

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

### 5. Update tests

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

1. **TypeScript: add `runPipeline`** — Add `runPipeline` to `run.ts`. Export from `index.ts`.
2. **TypeScript: delete `WorkflowBuilder` and `RunnableConfig`** — Remove from `ast.ts`.
3. **Update demos** — Replace `workflowBuilder().workflow(() => ...).run()` with `runPipeline(...)` in all four demos.
4. **Update tests** — Replace `workflowBuilder()` usage in `types.test.ts` and `round-trip.test.ts` with `config()`.

## Deferred

- **Rust-level input parameter**: Add `--input <json>` CLI flag, `input: Value` parameter to `run_workflow`, and a with-input overload on `runPipeline`. This would allow pipelines with concrete input types to receive their input from the caller rather than from a `constant()` node. All current demos use `constant()` for input, so this is not blocking.
