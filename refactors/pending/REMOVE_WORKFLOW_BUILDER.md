# Remove WorkflowBuilder

## Motivation

`workflowBuilder().workflow(() => pipeline).run()` is three layers of indirection around what should be a single function call. `WorkflowBuilder` creates a `RunnableConfig`, which wraps a `Config`, which wraps the pipeline action. The builder adds no value: there's one workflow per config, no configuration steps between `workflowBuilder()` and `.workflow()`, and no reuse of the builder instance.

Replace with `runPipeline(pipeline)` (or `runPipeline(pipeline, input)` when the pipeline expects input).

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

### Rust

**CLI** (`crates/barnum_cli/src/main.rs:26-38`):

```rust
Run {
    #[arg(long)]
    config: String,
    #[arg(long)]
    executor: String,
    #[arg(long)]
    worker: String,
}
```

**Event loop** (`crates/barnum_event_loop/src/lib.rs:300-301`):

```rust
let root = workflow_state.workflow_root();
advance(workflow_state, root, Value::Null, None).expect("initial advance failed");
```

The initial input is hardcoded to `Value::Null`.

## Proposed changes

### 1. Add `runPipeline` to `run.ts`

Two overloads: one for void-input pipelines (no second argument), one for pipelines that require input.

```typescript
// libs/barnum/src/run.ts

// Overload: pipeline with void input (all current demos)
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
  await run({ workflow: pipeline as WorkflowAction }, input);
}
```

When a pipeline has `__in?: void` (or `never` or `any`), overload 1 matches and no input is needed. When a pipeline has a concrete input type like `string`, only overload 2 matches, making `input` required.

### 2. Update `run()` to accept input

```typescript
// libs/barnum/src/run.ts

export function run(config: Config, input?: unknown): Promise<void> {
  // ...existing binary/executor/worker resolution...
  const configJson = JSON.stringify(config);

  const args = [
    "run",
    "--config", configJson,
    "--executor", executor,
    "--worker", worker,
  ];
  if (input !== undefined && input !== null) {
    args.push("--input", JSON.stringify(input));
  }

  // ...spawn child process with args...
}
```

### 3. Add `--input` to CLI

```rust
// crates/barnum_cli/src/main.rs

Run {
    #[arg(long)]
    config: String,
    #[arg(long)]
    executor: String,
    #[arg(long)]
    worker: String,
    /// Optional JSON input value for the workflow root.
    #[arg(long)]
    input: Option<String>,
}
```

```rust
async fn run(
    config_json: &str,
    executor: &str,
    worker: &str,
    input_json: Option<&str>,
) -> Result<(), Box<dyn std::error::Error>> {
    let config: barnum_ast::Config = serde_json::from_str(config_json)?;
    let input: Value = match input_json {
        Some(json) => serde_json::from_str(json)?,
        None => Value::Null,
    };
    let flat_config = flatten(config)?;
    let mut workflow_state = WorkflowState::new(flat_config);
    let mut scheduler = Scheduler::new(executor.to_owned(), worker.to_owned());

    let result = run_workflow(&mut workflow_state, &mut scheduler, input).await?;
    // ...
}
```

### 4. Pass input through `run_workflow`

```rust
// crates/barnum_event_loop/src/lib.rs

pub async fn run_workflow(
    workflow_state: &mut WorkflowState,
    scheduler: &mut Scheduler,
    input: Value,  // was hardcoded Value::Null
) -> Result<Value, RunWorkflowError> {
    let compiled_schemas = compile_schemas(workflow_state)?;
    let root = workflow_state.workflow_root();
    advance(workflow_state, root, input, None).expect("initial advance failed");
    // ...rest unchanged...
}
```

### 5. Delete `WorkflowBuilder` and `RunnableConfig`

Remove from `libs/barnum/src/ast.ts`:
- `WorkflowBuilder` interface (lines 971-974)
- `workflowBuilder()` function (lines 976-983)
- `RunnableConfig` class (lines 950-969)

Keep the `config()` helper function (line 939-941) for the type and round-trip tests.

### 6. Update exports

```typescript
// libs/barnum/src/index.ts

// Remove: workflowBuilder is no longer exported (it's deleted)
// Add:
export { run, runPipeline } from "./run.js";
```

### 7. Update demos

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
      (throwError) => pipe(/* ... */),
      logError.then(recur),
    ),
  ),
);
```

**`demos/convert-folder-to-ts/run.ts`:**
```typescript
await runPipeline(
  pipe(setup, listFiles.forEach(migrate({ to: "Typescript" })).drop(), typeCheckFix),
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
    forEach(withResource({ create: createBranchWorktree, action: implementAndReview, dispose: deleteWorktree })),
  ),
);
```

### 8. Update tests

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

### 9. Update event loop tests

The event loop tests in `crates/barnum_event_loop/src/lib.rs` call `run_workflow`. After adding the `input` parameter, these tests pass `Value::Null`:

```rust
// Before
let result = run_workflow(&mut workflow_state, &mut scheduler).await.unwrap();

// After
let result = run_workflow(&mut workflow_state, &mut scheduler, Value::Null).await.unwrap();
```

## Task list

1. **Rust: add `input` parameter to `run_workflow`** — Change signature, pass to `advance` instead of hardcoded `Value::Null`. Update event loop tests. Update CLI `run()` to pass `Value::Null` (preserve current behavior).
2. **Rust: add `--input` CLI flag** — Optional `--input <json>` argument. Parse and pass to `run_workflow`.
3. **TypeScript: add `runPipeline`, update `run()`** — Add overloaded `runPipeline` to `run.ts`. Add `input` parameter to `run()`. Pass `--input` to CLI when input is provided. Export `runPipeline` from `index.ts`.
4. **TypeScript: delete `WorkflowBuilder` and `RunnableConfig`** — Remove from `ast.ts`. Remove `workflowBuilder` from `index.ts` exports (it's an `export *` so just deleting the source suffices).
5. **Update demos** — Replace `workflowBuilder().workflow(() => ...).run()` with `runPipeline(...)` in all four demos.
6. **Update tests** — Replace `workflowBuilder()` usage in `types.test.ts` and `round-trip.test.ts` with `config()`.
