# Pre-compilation and serialization

## Context

Currently, running a workflow involves:

1. TypeScript builds the AST (`runPipeline(pipeline)`)
2. `runPipeline` serializes the config to JSON and passes it to the Rust CLI
3. Rust CLI reads JSON, deserializes, flattens, and creates `WorkflowState`
4. The event loop drives execution

Step 1-3 happen every time. For workflows that don't change between runs (the common case), this is redundant work.

## Proposal: a compiled, reusable workflow value

### `compile()` — constructor for `CompiledWorkflow`

`compile()` is a postfix method on a pipeline (`TypedAction`, alongside the existing `iterate`, `collect`, `bindInput`). It is a **constructor**: it produces a `CompiledWorkflow` instance.

It is only defined on pipelines whose input is `null` — a workflow that still needs an input isn't runnable, so it isn't compilable. The method is gated with a `this` type:

```ts
// On TypedAction<In, Out>:
compile(this: TypedAction<null, Out>): CompiledWorkflow<TypedAction<null, Out>>;
```

A pipeline that still wants input has `In != null`; you must satisfy that input first (`bindInput` / `call` / `chain(constant(x), …)` until `In` is `null`). Only then does `compile()` — and therefore `run()` — typecheck. This is what removes the `run(input)` overload entirely: input is supplied by narrowing `In` to `null` *before* compiling, never as a `run()` argument.

```ts
const compiled_workflow: CompiledWorkflow<TypedAction<null, Out>> = pipeline.compile();
```

What `compile()` does, concretely:

1. Walks the in-memory `Action` AST the user built.
2. Produces the `Config` (`{ workflow: Action }`) — the same object `runPipeline` builds today at `run.ts:132`.
3. Serializes it to the JSON AST (`JSON.stringify(config)`, as in `spawnBarnum`).

The result is **not** a free-floating JSON blob — it is a `CompiledWorkflow` that owns the serialized form and carries the pipeline's output type as a phantom parameter. It is a `class`, so it can carry the `fromJSON` static constructor:

```ts
class CompiledWorkflow<TPipeline extends Action> {
  // The serialized config JSON. The compiled artifact, exposed directly.
  readonly configJson: string;
  // Phantom — carries ExtractOutput<TPipeline> so run() stays typed.
  declare readonly __output?: ExtractOutput<TPipeline>;

  // Wrap an existing config-JSON string (e.g. read back from disk). No AST work.
  static fromJSON(configJson: string): CompiledWorkflow<Action>;

  // No `input` param: a CompiledWorkflow only exists for null-input pipelines.
  run(options?: RunOptions): Promise<ExtractOutput<TPipeline>>;
}
```

`pipeline.compile()` builds a `CompiledWorkflow` from the in-memory TypeScript AST.

The framework owns no file I/O — not writing, not reading. `compile()` exposes the serialized form as `configJson`, a plain string. If you want to persist it, you write it yourself (`fs.writeFileSync("workflow.barnum.json", compiled.configJson)`). If you want to run a previously-saved one, you read the file yourself and rebuild a `CompiledWorkflow` from the raw JSON — the `CompiledWorkflow` is the single thing you `run()`, no matter how you got the JSON:

```ts
const configJson = fs.readFileSync("workflow.barnum.json", "utf8");
await CompiledWorkflow.fromJSON(configJson).run();
```

`CompiledWorkflow.fromJSON(configJson)` wraps an existing config-JSON string into a `CompiledWorkflow` (no AST work — it just holds the string). `pipeline.compile()` is the other way to get one; both yield the same value with the same `run()`. There is no file-based API — you bring the string, the `CompiledWorkflow` runs it.

The JSON path can't recover `ExtractOutput` (there's no `Action` to read it from), so `CompiledWorkflow.fromJSON(json)` returns `CompiledWorkflow<Action>` and `run()` resolves to `unknown` — the expected tradeoff for the no-TypeScript escape hatch.

The serialized form is:
- Deterministic (same AST → same JSON, already guaranteed by `flatten` on the Rust side)
- Just a string you can write, inspect, or check into version control yourself
- Reusable across `run()` calls without re-walking the AST

### `run()` — method on `CompiledWorkflow`

`run()` is an instance method on `CompiledWorkflow`. Internally it reuses the existing `spawnBarnum` path (`run.ts:138`): resolve the binary, hand the engine the config JSON, parse the final value from stdout. (The temp-file the CLI is fed through is an internal handoff detail, not a user-facing artifact — unrelated to the "no file I/O" rule above, which is about the public API.) Because the instance carries `TPipeline`, the returned `Promise<ExtractOutput<TPipeline>>` stays fully typed.

```ts
const result = await pipeline.compile().run();
```

Like `compile()`, `run()` is only reachable once the pipeline's input is `null`. Supply any required input first, then run:

```ts
const result = await foo.call(constant.number(123)).run();
//                   ^ In becomes null here, so .compile()/.run() typecheck
```

`pipeline.run(...)` is sugar for `pipeline.compile().run(...)` — it builds the `CompiledWorkflow` and immediately invokes its `run`, and carries the same `this: TypedAction<null, Out>` gate.

The free `runPipeline` function is **deleted** — no back-compat. `pipeline.run()` and `pipeline.compile().run()` replace it. Input was previously a `runPipeline` argument; it no longer exists as one. Narrow the pipeline's input to `null` first (`foo.call(constant.number(123))`), then run.

## What the compiled artifact contains

`configJson` is the serialized `Config` (`{ workflow: Action }`). Rust-side, `barnum_ast::flat::flatten` turns this into `FlatConfig`, which is already a compact, self-contained representation:

```rust
pub struct FlatConfig {
    pub actions: Vec<FlatAction>,      // All nodes, indexed by ActionId
    pub handlers: Vec<HandlerKind>,    // All handlers, indexed by HandlerId
    pub workflow_root: ActionId,       // Entry point
    pub steps: HashMap<StepName, ActionId>,  // Named steps
}
```

Each `FlatAction` references handlers and child actions by ID (u32 indices), not by nested structure. This is already "compiled" — it's a flat instruction array similar to bytecode.

## Scope

This doc is **userland only** — the postfix `compile()`/`run()` surface in `libs/barnum`, no engine changes. `compile()` builds the `Config` and serializes it (work `runPipeline`/`spawnBarnum` already do); `run()` spawns the CLI as today.

Other work lives in separate docs:
- **Incremental compilation / caching** (skip re-walking the AST when source is unchanged) — userland follow-on. See `INCREMENTAL_COMPILE.md`.
- **Resumption / checkpointing** (serializing `WorkflowState`, resume-from-checkpoint) — requires `barnum_engine` changes. See `RESUMPTION.md`.
- **Contextual effects** (declared env/file reads resolved by the runtime) — requires `HandlerKind` and Rust runtime changes. See `RESUMPTION.md`.

## Direction

Postfix API is approved: `pipeline.compile().run()` with `.run()` as shorthand for `.compile().run()`. This is userland (no engine changes). Default export pipeline handling needs a separate doc. Postfix is ready to implement.