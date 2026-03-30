# Phase 2a: Sequential Bindings in Declare

## Goal

Extend `declare` to support sequential bindings where a binding can depend on previously-bound values. Phase 2 implements concurrent-only bindings; this phase adds the function form that enables dependencies between bindings.

## Prerequisites

Phase 2 (Variable Declarations) complete.

## Surface syntax

A binding can be a function that receives an array of all previously-bound VarRefs:

```ts
declare([
  exprA,
  ([a]) => exprB_using_a,
], ([a, b]) => body)
```

`exprA` runs first. Then the function receives `[a]` (a VarRef for exprA's result) and returns an action that may Perform to read `a`. The body receives VarRefs for both.

### Examples

**Sequential binding (depends on previous):**

```ts
declare([
  fetchUser,
  ([user]) => fetchReposForUser(user),
], ([user, repos]) =>
  pipe(repos, forEach(processRepo))
)
```

`fetchUser` runs first. Then `fetchReposForUser` receives a VarRef for user and runs. The body receives VarRefs for both.

**Mixed concurrent and sequential:**

```ts
declare([
  fetchUser,
  fetchConfig,
  ([user, config]) => deriveSettings(user, config),
], ([user, config, settings]) =>
  body
)
```

`fetchUser` and `fetchConfig` run concurrently (neither is a function). `deriveSettings` runs after both complete (it's a function that receives previous VarRefs).

## Grouping rules

The TS macro splits the binding array into groups:

1. **Leading non-function items** form a concurrent group (evaluated in All).
2. **Each function item** forms a sequential step (evaluated after all previous bindings).
3. **Non-function items after a function** form another concurrent group.

## Compilation

```
declare([
  exprA,
  ([a]) => exprB_using_a,
], ([a, b]) => body)

// Pseudo-AST notation (same as Phase 2):
//   Handle(effectId, stateInit, handler, body)
//   readVarHandler = Chain(ExtractField("state"), Tag("Resume"))

// Compiles to:
Chain(
  All(exprA, Identity),                                          // → [valA, pipeline_input]
  Handle(effectId_a, ExtractIndex(0), readVarHandler,
    Chain(
      All(Chain(ExtractIndex(1), exprB_using_a), ExtractIndex(1)),  // → [valB, pipeline_input]
      Handle(effectId_b, ExtractIndex(0), readVarHandler,
        Chain(ExtractIndex(1), body)                               // body gets pipeline_input
      )
    )
  )
)
```

Each sequential step adds a nested Handle. Each Handle stores exactly one binding's value (per-binding-effectId, same as Phase 2). Between sequential steps, an `All` preserves `pipeline_input` alongside the new binding's value so both remain available downstream.

## Type changes

The `declare` signature changes to accept both forms:

```ts
function declare<TIn, TBindings extends (Pipeable<TIn, any> | ((vars: VarRef<any>[]) => Pipeable<TIn, any>))[], TOut>(
  bindings: [...TBindings],
  body: (vars: InferVarRefs<TBindings>) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TOut>
```

`InferVarRefs` must handle both `Pipeable` (extract output type directly) and function form (extract output type from the returned Pipeable).

## Tests

1. Sequential binding (function in array) produces nested Chain + Handle.
2. Mixed concurrent/sequential produces correct grouping.
3. Sequential binding can Perform to read previous bindings.
4. Multiple sequential steps produce correctly nested Handles.
5. VarRef types inferred correctly through function form.

## Deliverables

1. Extend `declare()` to accept function bindings
2. Grouping logic (split array into concurrent/sequential groups)
3. Nested Handle compilation for sequential groups
4. Updated `InferVarRefs` type to handle function form
5. Tests per above
