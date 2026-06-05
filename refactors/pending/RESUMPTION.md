# Resumption and checkpointing

Engine-side companion to `PRECOMPILATION.md`. Everything here requires
`barnum_engine` / Rust runtime changes and is out of scope for the userland
postfix `compile()`/`run()` work.

## Serialization for resumption

### Problem

If a workflow is interrupted (crash, timeout, manual stop), it currently loses all progress. To resume, it would need to re-run from the beginning.

### State snapshot

The `WorkflowState` contains everything needed to resume:
- The `FlatConfig` (static, comes from the compiled config JSON)
- The frame stack (which actions are in progress, their completion state)
- Pending dispatches (which handlers need to run)
- Accumulated results (partial parallel/forEach results)

Serializing this state to JSON/bincode after each completion step would allow resumption: after each handler completion, write the state to a checkpoint location; on startup, if a checkpoint exists, load it and resume from where it left off.

### Idempotency requirement

Resumption only works if handlers are idempotent or the workflow is designed for at-least-once execution. A handler that creates a PR should check if the PR already exists before creating a new one.

This is the user's responsibility — the framework provides the mechanism (checkpoint + resume), the user provides the guarantee (idempotent handlers).

### Implementation note

Need to make `WorkflowState` serializable. It currently contains non-serializable tokio primitives in the event-loop layer, but the core state in `barnum_engine` is pure data. Medium effort.

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
