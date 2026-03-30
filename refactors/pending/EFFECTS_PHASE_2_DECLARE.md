# Phase 2: Variable Declarations (ReadVar Effect)

## Goal

Implement `declare` as the first real algebraic effect. This exercises the resume path: a Perform fires, the Handle reads from its opaque state and Resumes with the value. End-to-end validation that the Phase 1 substrate works for data flow.

**Phase 2 covers concurrent bindings only.** Sequential bindings (where a binding depends on previously-bound values) are deferred to Phase 2a — see `EFFECTS_PHASE_2A_SEQUENTIAL_DECLARE.md`.

## Prerequisites

Phase 1 (Effect Substrate) complete.

## Surface syntax

The canonical form is an array of bindings followed by a body callback:

```ts
declare([exprA, exprB], ([a, b]) => body)
```

All bindings are actions (Pipeable) — evaluated concurrently with the declare's pipeline input.

The body callback receives an array of all VarRefs, one per binding. VarRefs are destructured positionally.

### Example

```ts
declare([fetchUser, fetchConfig], ([user, config]) =>
  pipe(user, processWithConfig(config))
)
```

Both `fetchUser` and `fetchConfig` run concurrently. The body receives VarRefs for both.

## How declare compiles

Each binding gets its own effectId — the natural HOAS representation where each binder creates a fresh name. This avoids the dynamic typing problem that a shared effectId with runtime index would introduce (see "Why not a shared effectId" below).

```
declare([fetchUser, fetchConfig], ([user, config]) => body)

// user   = Perform(effectId_0)   — no payload
// config = Perform(effectId_1)   — no payload

// Pseudo-AST notation:
//   Handle(effectId, handler, body)
//   readVar(n) = Chain(ExtractField("state"), ExtractIndex(n), Tag("Resume"))

// Compiles to:
Chain(
  All(fetchUser, fetchConfig, Identity),       // → [User, Config, Input]
  Handle(effectId_0, readVar(0),
    Handle(effectId_1, readVar(1),
      Chain(ExtractIndex(2), body)             // body gets pipeline_input
    )
  )
)
```

`All(fetchUser, fetchConfig, Identity)` evaluates all bindings concurrently AND preserves the pipeline input (via Identity). The result is a tuple `[User, Config, pipeline_input]`.

Handle initializes its state to the pipeline value (a one-line engine change: `state: None` → `state: Some(value.clone())`). All nested Handles receive the same tuple as state. Each handler extracts the bound value at its known index: `readVar(n)` does `ExtractField("state") → ExtractIndex(n) → Tag("Resume")`. The Perform carries no payload — the effectId alone identifies which binding.

N bindings produce N nested Handle frames. This is the natural representation of N lexical bindings — `let a = ... in let b = ... in body`. Each `let` is a scope, each Handle is a scope.

### Why not a shared effectId

An earlier design used one shared effectId with the binding index as payload and an `ExtractDynamic` builtin to do runtime index lookup into the state tuple. This is dynamically typed — `extractDynamic([index, [User, Config]])` returns `unknown` because the index is a runtime value and each element has a different type. The per-binding-effectId design eliminates this: each handler extracts a statically-known index, so the types are preserved.

### Engine change: Handle initializes state from pipeline value

The only engine change needed: when creating a Handle frame, set `state: Some(value.clone())` instead of `state: None`. The body still receives the pipeline value unchanged. This is a one-line change in the `FlatAction::Handle` arm of `advance()`.

## VarRef: generic over the bound type

`VarRef<TValue>` wraps `TypedAction<never, TValue>`. Input is `never` because VarRefs don't consume pipeline input — they perform an effect. Output is `TValue`, the concrete type of the bound value.

Because `declare` is a generic function call, TypeScript infers `TValue` from each binding expression:

```ts
declare(
  [computeName, computeCount],
  // TypeScript infers: [VarRef<string>, VarRef<number>]
  ([name, count]) => pipe(
    name,                 // produces string
    appendCount(count),   // count produces number
  ),
)
```

Each VarRef carries the concrete type of its binding. No manual annotations, no `unknown` casts. The HOAS callback is what makes this work.

### Implementation sketch

```ts
type VarRef<TValue> = TypedAction<never, TValue>;

function declare<TIn, TBindings extends Pipeable<TIn, any>[], TOut>(
  bindings: [...TBindings],
  body: (vars: InferVarRefs<TBindings>) => Pipeable<TIn, TOut>,
): TypedAction<TIn, TOut> {
  const effectIds = bindings.map(() => generateUniqueId());
  const varRefs = effectIds.map((id) => createVarRef(id));
  const bodyAst = body(varRefs as any);
  return compileToNestedHandles(bindings, effectIds, bodyAst);
}
```

The key type: `InferVarRefs<TBindings>` maps each binding to `VarRef<OutputOf<binding>>`. TypeScript resolves `OutputOf` from each binding expression's output type.

**Note:** Phase 2 only accepts `Pipeable` bindings (concurrent). Phase 2a adds function bindings for sequential dependencies.

## The HOAS pattern

Each binding gensyms its own fresh `EffectId`. Each VarRef is a `Perform(effectId)` node with no payload — the effectId alone identifies the binding. TypeScript's lexical scoping ensures VarRefs can only be used within the callback body.

Per the HOAS pattern:
1. Gensym a fresh `EffectId` **per binding**
2. Create nested Handles, one per binding, each keyed on its effectId
3. Provide `Perform(effectId_i)` wrappers to the callback as opaque `Pipeable` nodes

For nested declares, each binding across all declares has its own `EffectId`. Inner VarRefs are caught by the nearest matching Handle. Outer VarRefs bubble past inner Handles (wrong effectId) and are caught by the outer Handle.

## What this replaces

| Current pattern | With declare |
|---|---|
| `augment(pipe(extract, transform))` | Bind the result, reference it later via VarRef |
| `tap(sideEffect)` (to preserve context) | Side effect in body, context from VarRef |
| `withResource({ create, action, dispose })` | Bind the resource (dispose comes in Phase 5) |
| `pick("field1", "field2")` (to narrow for invariance) | Still needed — VarRef gives the full value, pick narrows it |

`augment` and `tap` become unnecessary for context threading. `pick` remains necessary for handler input narrowing (invariance at serialization boundaries).

## Test strategy

### TypeScript compilation tests

1. Single-binding declare produces correct Chain + Handle AST.
2. Two concurrent bindings produce All + nested Handles.
3. VarRef type checking: binding type matches VarRef output type.

### Rust scheduler tests

1. **Simple declare**: One Handle with state = `42`. Perform fires. Handler resumes with state. Verify body receives 42.
2. **Multiple bindings**: Two nested Handles with state = `1` and `2`. Two Performs in a Chain. Verify each returns correct value from its Handle.
3. **Nested declares**: Inner declare's Handle has state = `"inner"`, outer's has state = `"outer"`. Inner VarRef caught by inner Handle. Outer VarRef bubbles past inner, caught by outer.
4. **Concurrent bindings**: All evaluates two expressions. Nested Handles each extract their binding. Verify correct variable resolution.
5. **Declare inside ForEach**: Each iteration enters its own Handle frames. Verify isolation.
6. **Declare inside All**: Two branches reference the same outer declare. Verify both get correct values.

### Demo migration

Rewrite the `identify-and-address-refactors` demo to use `declare` instead of `tap`/`augment`/`pick` for context threading. The demo should be shorter and clearer.

## No new builtins

The per-binding-effectId design uses only existing builtins (`ExtractIndex`, `ExtractField`, `Tag`). The `ExtractDynamic` builtin from the earlier shared-effectId design is no longer needed.

## Deliverables

1. `declare()` TypeScript function (concurrent bindings only, per-binding effectId)
2. VarRef TypedAction construction (Perform with binding-specific effectId, no payload)
3. Nested Handle compilation (one Handle per binding, trivial handler)
4. Tests per above
5. Demo migration
