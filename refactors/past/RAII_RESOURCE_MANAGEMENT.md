# RAII resource management: withResource combinator

## Problem

Some workflows need to acquire a resource, use it, and guarantee cleanup regardless of the action's outcome. The canonical example: git worktrees (create before work, delete after PR creation).

Without a dedicated pattern, the create and dispose steps are manually wired into the pipeline, and there's no structural guarantee that dispose runs after create.

## Current implementation

`withResource` in `builtins.ts`:

```ts
function withResource<TIn, TResource, TOut>({
  create,
  action,
  dispose,
}: {
  create: TypedAction<TIn, TResource>;
  action: TypedAction<TResource, TOut>;
  dispose: TypedAction<TOut, any>;
}): TypedAction<TIn, never>
```

Desugars to: `chain(create, chain(action, dropResult(dispose)))`.

### Convention

The `action` must thread resource identity (e.g., worktree path) through to its output so `dispose` can access it. This is the caller's responsibility — the type system enforces that `dispose` accepts `TOut`, but it's up to the action to include resource metadata in `TOut`.

Example from `identify-and-address-refactors`:
- `create`: `Refactor → WorktreeContext` (creates worktree, returns path + branch)
- `action`: `WorktreeContext → PRResult` (implements, commits, creates PR — PRResult includes `worktreePath`)
- `dispose`: `PRResult → void` (deletes worktree using `prUrl` + `worktreePath`)

## Limitation: action result is lost

`withResource` returns `never` because dispose's result is discarded via `dropResult`. The surrounding pipeline cannot use the action's output (e.g., the PR URL).

This is acceptable when `withResource` is used inside `forEach` (the outer pipeline doesn't need individual results) but limiting when you want to collect results.

## Future: preserving the action result

To return `TOut` while still running dispose, we'd need:

### Option A: tee combinator

```ts
// Run action, then fork: one branch continues with TOut,
// the other runs dispose for side effects.
function tee<In, Out>(
  sideEffect: TypedAction<Out, any>,
): TypedAction<Out, Out>
```

Then: `chain(create, chain(action, chain(tee(dispose), ...)))`.

This requires a `tee` builtin in the Rust scheduler that clones the value, runs the side effect on the clone, and passes the original through.

### Option B: parallel with identity

```ts
chain(create, chain(action, chain(
  parallel(identity(), dispose),
  extractIndex(0),  // doesn't exist yet
)))
```

This requires an `extractIndex` builtin (or a more general tuple destructuring combinator).

### Option C: higher-order combinator in the scheduler

The scheduler could natively understand `WithResource { create, action, dispose }` as a first-class AST node, handling the result threading internally. This is the cleanest semantics but adds complexity to the Rust AST and scheduler.

## When to use

Use `withResource` when:
- A resource has a clear create/dispose lifecycle
- The dispose step must run after the action completes
- The action's result is either not needed downstream or is consumed within the `withResource` scope (e.g., inside `forEach`)

Don't use `withResource` when:
- You need the action's result after cleanup — wire create/dispose manually instead
- The "resource" is just setup/teardown with no identity to thread through
