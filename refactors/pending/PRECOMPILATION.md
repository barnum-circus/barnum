# Pre-compilation and serialization

## Context

Currently, running a workflow involves:

1. TypeScript builds the AST (`runPipeline(pipeline)`)
2. `runPipeline` serializes the config to JSON and passes it to the Rust CLI
3. Rust CLI reads JSON, deserializes, flattens, and creates `WorkflowState`
4. The event loop drives execution

Step 1-3 happen every time. For workflows that don't change between runs (the common case), this is redundant work.

## Proposal: pre-compiled workflow files

### Compile step

```bash
barnum compile workflow.ts -o workflow.barnum.json
```

Runs the TypeScript config builder, captures the JSON AST, and writes it to a file. This file is the "compiled" workflow — it contains the flattened `FlatConfig` ready for the engine.

The `.barnum.json` file is:
- Deterministic (same input → same output, already guaranteed by `flatten`)
- Checkable into version control
- Loadable without a TypeScript runtime

### Run step

```bash
barnum run workflow.barnum.json
```

Skips TypeScript entirely. Reads the pre-compiled `FlatConfig`, creates `WorkflowState`, runs the event loop.

### Incremental: only recompile when source changes

```bash
barnum run workflow.ts --cache .barnum-cache/
```

Hashes the TypeScript source + handler files. If the hash matches the cached `.barnum.json`, skip compilation. Otherwise, recompile and update the cache.

This is analogous to how `tsc --incremental` works — the cached artifact is a transparent optimization.

## What the compiled format contains

The `FlatConfig` (from `barnum_ast::flat::flatten`) is already a compact, self-contained representation:

```rust
pub struct FlatConfig {
    pub actions: Vec<FlatAction>,      // All nodes, indexed by ActionId
    pub handlers: Vec<HandlerKind>,    // All handlers, indexed by HandlerId
    pub workflow_root: ActionId,       // Entry point
    pub steps: HashMap<StepName, ActionId>,  // Named steps
}
```

Each `FlatAction` references handlers and child actions by ID (u32 indices), not by nested structure. This is already "compiled" — it's a flat instruction array similar to bytecode.

## Serialization for resumption

### Problem

If a workflow is interrupted (crash, timeout, manual stop), it currently loses all progress. To resume, it would need to re-run from the beginning.

### State snapshot

The `WorkflowState` contains everything needed to resume:
- The `FlatConfig` (static, can come from the pre-compiled file)
- The frame stack (which actions are in progress, their completion state)
- Pending dispatches (which handlers need to run)
- Accumulated results (partial parallel/forEach results)

Serializing this state to JSON/bincode after each completion step would allow resumption:

```bash
barnum run workflow.barnum.json --checkpoint .barnum-state/
```

After each handler completion, write the state to the checkpoint directory. On startup, if a checkpoint exists, load it and resume from where it left off.

### Idempotency requirement

Resumption only works if handlers are idempotent or the workflow is designed for at-least-once execution. A handler that creates a PR should check if the PR already exists before creating a new one.

This is the user's responsibility — the framework provides the mechanism (checkpoint + resume), the user provides the guarantee (idempotent handlers).

## Contextual effects for reading input

### Problem

Some handlers need input that isn't part of the pipeline data flow. Examples:
- Environment variables (`GITHUB_TOKEN`)
- CLI arguments (`--dry-run`)
- Configuration files (`.env`, `tsconfig.json`)
- User prompts (interactive input)

Currently, handlers read these directly (e.g., `process.env.GITHUB_TOKEN` in the handler code). This works but is invisible to the workflow — there's no way to validate, mock, or log these reads.

### Proposal: effect system for external reads

```ts
// In the handler definition
export default createHandler({
  effects: {
    env: ["GITHUB_TOKEN", "DRY_RUN"],
    files: ["tsconfig.json"],
  },
  handle: async ({ value, env, files }) => {
    // env.GITHUB_TOKEN is typed string | undefined
    // files["tsconfig.json"] is the file contents
  },
});
```

The runtime resolves effects before invoking the handler:
1. Reads the requested environment variables
2. Reads the requested files
3. Passes them as typed arguments to `handle`

Benefits:
- Workflow can be analyzed for required effects without running it
- Testing: mock effects instead of setting real env vars
- Logging: the runtime knows what external state each handler reads
- Caching: if effects haven't changed, handler output might be cacheable

### Implementation complexity

High. This requires:
- New fields on `HandlerKind` for effect declarations
- The Rust runtime resolving effects before dispatch
- TypeScript type inference for effect parameters in `handle`
- Serialization of effect values as part of the handler protocol

This is a significant extension. It should wait until the core workflow algebra is stable and real users are running production workflows.

## Priority

1. **Pre-compiled workflow files** — Low effort, immediate value. The `FlatConfig` is already JSON-serializable. Just need a `compile` CLI subcommand.
2. **Resumption checkpoints** — Medium effort. Need to make `WorkflowState` serializable (it currently contains non-serializable tokio primitives in the event loop layer, but the core state in `barnum_engine` is pure data).
3. **Contextual effects** — High effort, speculative value. Defer until real usage reveals the need.
