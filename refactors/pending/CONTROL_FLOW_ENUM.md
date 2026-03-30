# Control flow: LoopResult namespace and scope/exit

> **Convention**: All discriminated unions use `TaggedUnion<Def>` — every variant carries `{ kind: K; value: T; __def?: Def }`. This is not optional: **all union variants must carry `__def`**, no exceptions. Constructors like `recur()`, `done()`, `some()`, `none()`, and `tag()` all require the full variant map as a type parameter so the output type carries `__def`. Branch uses `ExtractDef` for inference and auto-unwraps `value` before each case handler.

## The types

### LoopResult

```ts
type LoopResultDef<TContinue, TBreak> = {
  Continue: TContinue;
  Break: TBreak;
};

type LoopResult<TContinue, TBreak> = TaggedUnion<LoopResultDef<TContinue, TBreak>>;
// = { kind: "Continue"; value: TContinue; __def?: LoopResultDef<TContinue, TBreak> }
// | { kind: "Break"; value: TBreak; __def?: LoopResultDef<TContinue, TBreak> }
```

Already exists in `ast.ts`. Used by `loop()` to drive iteration: `Continue` restarts the loop body with a new value, `Break` exits with a result.

### ScopeResult (proposed)

```ts
type ScopeResultDef<TExit, TContinue> = {
  Exit: TExit;
  Continue: TContinue;
};

type ScopeResult<TExit, TContinue> = TaggedUnion<ScopeResultDef<TExit, TContinue>>;
```

For early return from a `scope`. `Exit` short-circuits, `Continue` flows through to the next pipeline step. See "scope/exit" section below.

## API surface: the `LoopResult` namespace

Parallel to the `Option` namespace. All loop-related combinators live on `LoopResult.`.

```ts
import { LoopResult, loop } from "@barnum/barnum";

loop(
  pipe(
    healthCheck,
    branch({
      Healthy: LoopResult.done(),   // receives healthy payload, breaks loop
      Unhealthy: LoopResult.recur(), // receives unhealthy payload, continues loop
    }),
  ),
)
```

### Design principle: actions, not values

Same as Option — all arguments are actions. Rust doesn't have LoopResult (it uses `break`/`continue` keywords), but the principle applies to any combinator arguments.

## Constructors

### `LoopResult.recur` / `LoopResult.done`

Already exist as top-level `recur()` and `done()` exports. The namespace form is the preferred API:

```ts
LoopResult.recur<TContinue, TBreak>(): TypedAction<TContinue, LoopResult<TContinue, TBreak>>
// = tag<LoopResultDef<TContinue, TBreak>, "Continue">("Continue")
// Input: TContinue (feeds back as loop input), Output: full LoopResult

LoopResult.done<TContinue, TBreak>(): TypedAction<TBreak, LoopResult<TContinue, TBreak>>
// = tag<LoopResultDef<TContinue, TBreak>, "Break">("Break")
// Input: TBreak (the loop's output), Output: full LoopResult
```

Both require both type parameters to ensure `__def` carries the full variant map.

## Transforming

### `LoopResult.mapBreak` — transform the break value

```ts
LoopResult.mapBreak<TContinue, TBreak, U>(
  action: TypedAction<TBreak, U>,
): TypedAction<LoopResult<TContinue, TBreak>, LoopResult<TContinue, U>>
```

Apply `action` to the `Break` value, rewrap. Pass `Continue` through unchanged.

Desugars to:
```ts
branch({
  Break: pipe(action, LoopResult.done<TContinue, U>()),
  Continue: LoopResult.recur<TContinue, U>(),
})
```

### `LoopResult.mapContinue` — transform the continue value

```ts
LoopResult.mapContinue<TContinue, TBreak, U>(
  action: TypedAction<TContinue, U>,
): TypedAction<LoopResult<TContinue, TBreak>, LoopResult<U, TBreak>>
```

Apply `action` to the `Continue` value, rewrap. Pass `Break` through unchanged.

Desugars to:
```ts
branch({
  Continue: pipe(action, LoopResult.recur<U, TBreak>()),
  Break: LoopResult.done<U, TBreak>(),
})
```

### `LoopResult.inspect` / `LoopResult.inspectContinue`

Side effects on Break or Continue values without changing the result. Same pattern as `Option.inspect`.

```ts
LoopResult.inspect<TContinue, TBreak>(
  action: TypedAction<TBreak, unknown>,
): TypedAction<LoopResult<TContinue, TBreak>, LoopResult<TContinue, TBreak>>

LoopResult.inspectContinue<TContinue, TBreak>(
  action: TypedAction<TContinue, unknown>,
): TypedAction<LoopResult<TContinue, TBreak>, LoopResult<TContinue, TBreak>>
```

## Querying

### `LoopResult.isBreak` / `LoopResult.isContinue`

```ts
LoopResult.isBreak<TContinue, TBreak>(): TypedAction<LoopResult<TContinue, TBreak>, boolean>
LoopResult.isContinue<TContinue, TBreak>(): TypedAction<LoopResult<TContinue, TBreak>, boolean>
```

Desugar to:
```ts
// isBreak
branch({ Break: pipe(drop(), constant(true)), Continue: pipe(drop(), constant(false)) })
```

Rarely useful — you'd branch on `Break`/`Continue` directly. Present for completeness.

## Extracting

### `LoopResult.unwrapBreak`

```ts
LoopResult.unwrapBreak<TContinue, TBreak>(): TypedAction<LoopResult<TContinue, TBreak>, TBreak>
```

Extract the Break value. Panics on Continue. Blocked on error handling primitives.

### `LoopResult.unwrapBreakOr`

```ts
LoopResult.unwrapBreakOr<TContinue, TBreak>(
  defaultAction: TypedAction<TContinue, TBreak>,
): TypedAction<LoopResult<TContinue, TBreak>, TBreak>
```

Desugars to:
```ts
branch({
  Break: identity(),
  Continue: defaultAction,  // receives TContinue, produces TBreak
})
```

## The `loop` function

Already exists. Takes a body that produces `LoopResult<In, Out>` and extracts the Break value:

```ts
function loop<In, TOut extends LoopResult<any, any>, R extends string = never>(
  body: Pipeable<In, TOut, R>,
): TypedAction<In, ExtractBreakValue<TOut>, R>
```

### Closure form (from LOOP_WITH_CLOSURE.md)

The closure form provides scoped `recur`/`done` that are pre-bound to the loop's type parameters, avoiding the inference problem where `TContinue` must be specified manually:

```ts
loop(({ recur, done }) =>
  pipe(
    typeCheck,
    classifyErrors,
    branch({
      HasErrors: pipe(forEach(fix), drop(), recur()),  // receives TypeError[]
      Clean: done(),  // receives void
    }),
  ),
)
```

The closure is called at AST construction time. The AST shape is identical — `recur()` and `done()` produce the same Tag nodes. The difference is purely type-level: the closure can infer `TContinue` and `TBreak` from context.

See LOOP_WITH_CLOSURE.md for full design.

## scope / exit (proposed)

`scope` is the generalization of `loop`. Where `loop` repeats on Continue and exits on Break, `scope` just exits on Exit and continues the pipeline otherwise.

### The `?` operator

This is the killer use case. In Rust, `?` propagates errors up to the enclosing function. In barnum, `scope` + `exit` does the same:

```ts
scope(({ exit }) =>
  pipe(
    tryAction(step1),
    branch({ Ok: identity(), Err: exit() }),  // ? operator
    tryAction(step2),
    branch({ Ok: identity(), Err: exit() }),
    tryAction(step3),
    branch({ Ok: identity(), Err: exit() }),
  ),
)
// output: last Ok value, or first Err value
```

### Scope semantics

```ts
function scope<In, TExit, TOut>(
  build: (ctx: {
    exit: () => TypedAction<TExit, ScopeResult<TExit, never>>;
  }) => TypedAction<In, TOut | ScopeResult<TExit, TOut>>,
): TypedAction<In, TExit | TOut>
```

- `exit()` tags a value as `Exit`, short-circuiting the rest of the pipeline
- Any value that reaches the end of the scope without hitting `exit()` is the normal result
- Output is `TExit | TOut` — the early-exit type unioned with the normal completion type

### Relationship between loop and scope

`loop` = `scope` + implicit restart on Continue:

```
loop(body)  ≡  scope(({ exit }) =>
                 pipe(
                   body,
                   branch({
                     Continue: restart(),  // implicit in loop
                     Break: exit(),
                   }),
                 ),
               )
```

Whether to actually implement `loop` on top of `scope` or keep them as independent AST nodes is an implementation question. Independent nodes are simpler for the Rust engine. Building `loop` on `scope` is cleaner conceptually.

### Scope as an AST node

```rust
// Rust AST
pub struct ScopeAction {
    pub body: Box<Action>,
    pub scope_id: ScopeId,
}

// Exit is a special Tag that the engine recognizes
pub struct ExitAction {
    pub scope_id: ScopeId,
}
```

The `scope_id` connects `exit()` to its enclosing `scope`. Same mechanism as loop's `Continue`/`Break` but with only one signal (Exit). The engine unwinds frames back to the Scope frame when it sees an Exit tag.

## Sugar: `propagate`

A common pattern with scope/exit and Result:

```ts
// Without sugar:
scope(({ exit }) =>
  pipe(
    tryAction(step1),
    branch({ Ok: identity(), Err: exit() }),
    tryAction(step2),
    branch({ Ok: identity(), Err: exit() }),
  ),
)

// With propagate sugar:
scope(({ exit }) =>
  pipe(
    tryAction(step1), propagate(exit),
    tryAction(step2), propagate(exit),
  ),
)

// Where:
function propagate<T, E>(exit: () => TypedAction<E, ScopeResult<E, never>>)
  : TypedAction<Result<T, E>, T>
{
  return branch({ Ok: identity(), Err: exit() });
}
```

Or even higher-level: `tryScope` that automatically wraps each step:

```ts
tryScope(pipe(step1, step2, step3))
// Each step can "throw" by producing Err. Err propagates automatically.
```

## Priority

### Tier 1: already implemented

- `LoopResult` type (exists in `ast.ts`)
- `recur()` / `done()` constructors (exist in `builtins.ts`)
- `loop()` combinator (exists in `ast.ts`)

### Tier 2: namespace + closure

- `LoopResult` namespace object — mirrors existing top-level `recur`/`done`
- `loop` closure form — LOOP_WITH_CLOSURE.md
- `LoopResult.mapBreak`, `LoopResult.mapContinue` — transform variants

### Tier 3: scope/exit

- `scope` AST node + engine support
- `exit()` scoped constructor
- `propagate()` sugar for Result + scope
- `tryScope` high-level sugar

### Tier 4: nice to have

- `LoopResult.inspect` / `LoopResult.inspectContinue`
- `LoopResult.isBreak` / `LoopResult.isContinue`
- `LoopResult.unwrapBreak` / `LoopResult.unwrapBreakOr`

## Files to change

### For LoopResult namespace (tier 2)

| File | What changes |
|------|-------------|
| `libs/barnum/src/builtins.ts` | Add `LoopResult` namespace object. `recur()`/`done()` stay as top-level exports for backward compat; namespace is preferred API. |
| `libs/barnum/tests/types.test.ts` | Type-level tests for namespace combinators. |
| `libs/barnum/tests/patterns.test.ts` | Runtime tests for namespace AST shapes. |

### For loop closure form (tier 2)

| File | What changes |
|------|-------------|
| `libs/barnum/src/ast.ts` | Add overload to `loop()` accepting a builder function. |
| `libs/barnum/tests/types.test.ts` | Type inference tests for closure-based loop. |

### For scope/exit (tier 3)

| File | What changes |
|------|-------------|
| `libs/barnum/src/ast.ts` | Add `ScopeAction` to `Action` union. Add `scope()` combinator. |
| `libs/barnum/src/builtins.ts` | Add `exit()` constructor (or provide via scope closure). |
| Rust AST (when it exists) | Add `Action::Scope` variant. Add `ScopeId` newtype. |
| Rust engine (when it exists) | Add `Frame::Scope` variant. Handle Exit tag unwinding. |
| `libs/barnum/tests/` | Tests for scope/exit behavior. |

### Existing functions that don't change

- **`recur()` / `done()`** — Stay as top-level exports. `LoopResult.recur()` / `LoopResult.done()` are aliases on the namespace.
- **`loop()`** — Existing overload stays. Closure form is an additional overload.
- **`branch()`** — Already handles LoopResult via TaggedUnion/ExtractDef.
- **`tag()`** — `recur`/`done` build AST directly (don't call `tag()`). No change.
