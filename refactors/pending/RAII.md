# RAII as a Primitive

## Motivation

`withResource` currently handles the create/use/dispose pattern as a combinator — a TypeScript function that desugars into `all + merge + extractIndex` nodes. It works, but it's a complex encoding of a simple idea: "run cleanup when this scope exits."

What if RAII were a first-class AST primitive instead of a combinator-level encoding? What if handlers themselves could declare cleanup behavior?

## Current state: withResource as a combinator

`withResource({ create, action, dispose })` is pure sugar over existing AST nodes:

```
TIn → all(create, identity) → merge → action → all(extractResult, dispose) → extractResult
```

This encoding has several properties:
- Dispose runs after action completes successfully
- Dispose does NOT run if action fails (no error handling yet)
- The encoding is complex: 4 intermediate steps, multiple all nodes, extractIndex gymnastics
- Every user of RAII pays this AST complexity tax

## Proposal: RAII as an AST node

### Option 1: WithResource as a first-class action

Move the concept from combinator-land into the AST:

```ts
interface WithResourceAction {
  kind: "WithResource";
  create: Action;    // TIn → TResource
  action: Action;    // TResource & TIn → TOut
  dispose: Action;   // TResource → void (runs on scope exit, even on error)
}
```

The scheduler handles the create/merge/action/dispose lifecycle internally. Dispose is guaranteed to run — even if the action fails (once we have error handling).

This is what `withResource` already does, but as a scheduler primitive instead of a combinator encoding. The benefits:

1. **Simpler AST**: One node instead of nested Chain/All/Merge/ExtractIndex
2. **Guaranteed cleanup**: The scheduler can enforce dispose-on-exit, including on error paths
3. **Debuggability**: The scheduler sees "WithResource" in the frame tree, not an opaque chain of all+extract nodes
4. **Optimization**: The scheduler can special-case the lifecycle instead of executing the generic all/merge machinery

### Option 2: Scope + defer (generalized RAII)

Instead of a specific `WithResource` node, add two more general primitives:

```ts
interface ScopeAction {
  kind: "Scope";
  body: Action;
  deferred: Action[];  // run in reverse order on scope exit
}
```

Or alternatively, `Defer` as an inline action that registers cleanup:

```ts
interface DeferAction {
  kind: "Defer";
  cleanup: Action;     // registered for scope exit
  body: Action;        // continues with the pipeline value
}
```

Usage:

```ts
scope(
  pipe(
    createWorktree,       // → { worktreePath, branch }
    defer(deleteWorktree), // registers cleanup, passes value through
    implement,
    commit,
  ),
)
```

`defer` registers `deleteWorktree` to run when the enclosing `scope` exits, then passes the pipeline value through unchanged (like `tap` but for cleanup). Multiple `defer` calls stack — they run in reverse order on exit (LIFO, like Go's `defer`).

This is more general than `WithResource`:
- Multiple resources can be acquired and cleaned up independently
- Cleanup is ordered (LIFO)
- Works with any action, not just create/dispose pairs

### Option 3: Handlers with destructors

What if a handler could declare its own cleanup?

```ts
const createWorktree = createHandler({
  inputValidator: z.object({ branch: z.string() }),
  handle: async ({ value }) => {
    const path = await makeWorktree(value.branch);
    return { worktreePath: path, branch: value.branch };
  },
  dispose: async ({ value }) => {
    await removeWorktree(value.worktreePath);
  },
});
```

When the scheduler sees that a handler has a `dispose` function, it automatically registers cleanup for the handler's output. The output value carries a "needs cleanup" marker. When the scope containing that value exits, the scheduler runs `dispose` with the output value.

This is linear types / affine types in disguise: the handler's output is a resource that must be consumed (disposed). The scheduler tracks resource lifetimes automatically.

**Pros**: The create/dispose pairing is declared once, at the handler level. Callers don't need to think about cleanup — it happens automatically.

**Cons**: Implicit behavior. The caller doesn't see the cleanup in the pipeline. If a resource is passed to multiple steps (via `all` or variable references), when does dispose run? When all references are dead? That's garbage collection, not RAII.

### Comparison

| | WithResource (current) | AST node | Scope + defer | Handler destructors |
|---|---|---|---|---|
| Complexity for callers | Medium (combinator API) | Medium (same API, simpler internals) | Low (defer is inline) | Very low (automatic) |
| Scheduler changes | None | New action kind | New action kinds | Resource tracking |
| Multiple resources | Nest withResource calls | Nest nodes | Stack defers | Automatic |
| Error cleanup | No (not yet) | Yes (scheduler controls) | Yes (scope exit) | Yes (lifetime tracking) |
| Visibility | Explicit | Explicit | Explicit | Implicit |
| Composability | Good | Good | Better (independent defers) | Unclear (ownership semantics) |

## Interaction with let bindings

With `let` + `scope` + `defer`:

```ts
scope(
  let_({
    worktree: pipe(deriveBranch, createWorktree),
  }, ({ worktree }) =>
    pipe(
      defer(worktree.then(deleteWorktree)),
      worktree.then(implement),
      worktree.then(commit),
      worktree.then(createPR),
    ),
  ),
)
```

The `let` binding creates the worktree once. `defer` registers cleanup. The variable reference provides access to the resource throughout the body. `scope` delimits the cleanup boundary.

This is cleaner than `withResource` because:
- No special `create` / `action` / `dispose` separation
- The worktree is a named variable, not threaded through a pipeline
- Multiple resources compose naturally (multiple `let` bindings + `defer`s)
- The control flow is explicit and linear

This is essentially what Rust does with `let` + `Drop` + block scoping. The block is the scope, `let` creates the binding, `Drop` runs on scope exit.

## Relation to error handling

RAII is only meaningful once we have error handling. Without errors, dispose runs exactly when `withResource`'s current encoding runs it — after the action completes. The value of RAII is cleanup-on-failure.

Once `tryAction` exists (see MISSING_LANGUAGE_FEATURES.md), the interaction is:
- `scope` + `defer`: defer runs on normal exit AND on error exit
- `tryAction` inside a scope: error is caught, deferred cleanup still runs
- `tryAction` outside a scope: scope exits normally (action succeeded or error was caught), deferred cleanup runs

The ordering: error handling first, then RAII. RAII without error handling is equivalent to what we have today.

## Open questions

1. **Is withResource sufficient for now?** It handles the common case (one resource, linear lifecycle). The encoding is ugly internally but the API is clean. The main gap is error-path cleanup, which requires error handling regardless of RAII approach.

2. **Scope + defer vs WithResource AST node**: Scope + defer is more general but adds two new concepts. WithResource as an AST node is simpler but less flexible. The generality of scope + defer may not be needed if most workflows have at most one resource.

3. **Handler destructors**: Appealing but introduces implicit resource tracking. The "when does dispose run" question is hard in the presence of `all` and variable sharing. Linear types (must use exactly once) are the rigorous answer, but that's a heavy type system feature.

4. **Interaction with lazy let bindings**: If bindings are lazy and a binding creates a resource, does the thunk carry a dispose? If the thunk is never forced, no resource is created and no cleanup is needed. If it is forced, the resource exists and needs cleanup. This is natural — lazy evaluation + RAII means resources are created on-demand and cleaned up on scope exit.
