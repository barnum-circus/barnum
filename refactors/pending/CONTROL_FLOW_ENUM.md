# LoopResult<TContinue, TBreak>

## The type

```ts
type LoopResultDef<TContinue, TBreak> = { Continue: TContinue; Break: TBreak };
type LoopResult<TContinue, TBreak> = TaggedUnion<LoopResultDef<TContinue, TBreak>>;
```

Same pattern as `Option<T>` and `Result<TValue, TError>` — a `TaggedUnion` carrying `__def` so `.branch()` works via `ExtractDef`. Already exists in `ast.ts`.

Rust equivalent: `ControlFlow<B, C>`. `Continue` restarts the loop body with a new value, `Break` exits with a result.

## Constructors

### `LoopResult.recur`

```ts
LoopResult.recur<TContinue, TBreak>(): TypedAction<TContinue, LoopResult<TContinue, TBreak>>
// = tag<LoopResultDef<TContinue, TBreak>, "Continue">("Continue")
```

Already exists as top-level `recur()`. The namespace form is preferred.

### `LoopResult.done`

```ts
LoopResult.done<TContinue, TBreak>(): TypedAction<TBreak, LoopResult<TContinue, TBreak>>
// = tag<LoopResultDef<TContinue, TBreak>, "Break">("Break")
```

Already exists as top-level `done()`. The namespace form is preferred.

## Transforming

### `LoopResult.mapBreak` — transform the break value

```ts
LoopResult.mapBreak<TContinue, TBreak, U>(
  action: Pipeable<TBreak, U>,
): TypedAction<LoopResult<TContinue, TBreak>, LoopResult<TContinue, U>>
```

Apply `action` to the `Break` value, rewrap. Pass `Continue` through unchanged. Analogous to `Result.map`.

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
  action: Pipeable<TContinue, U>,
): TypedAction<LoopResult<TContinue, TBreak>, LoopResult<U, TBreak>>
```

Apply `action` to the `Continue` value, rewrap. Pass `Break` through unchanged. Analogous to `Result.mapErr`.

Desugars to:
```ts
branch({
  Continue: pipe(action, LoopResult.recur<U, TBreak>()),
  Break: LoopResult.done<U, TBreak>(),
})
```

## Extracting

### `LoopResult.unwrapBreakOr` — extract Break or compute default from Continue

```ts
LoopResult.unwrapBreakOr<TContinue, TBreak>(
  defaultAction: Pipeable<TContinue, TBreak>,
): TypedAction<LoopResult<TContinue, TBreak>, TBreak>
```

Takes an action that receives the Continue payload and produces a fallback. Analogous to `Result.unwrapOr`.

Desugars to:
```ts
branch({
  Break: identity(),
  Continue: defaultAction,
})
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

Rarely useful — branch on `Break`/`Continue` directly instead.

## Postfix methods on TypedAction

Gated by `this` parameter constraints (same pattern as `.mapOption()` and Result postfix methods).

All LoopResult method names are unique — no collision with Option or Result postfix names. No suffix needed.

| Postfix | Equivalent to |
|---|---|
| `.mapBreak(action)` | `LoopResult.mapBreak(action)` |
| `.mapContinue(action)` | `LoopResult.mapContinue(action)` |
| `.unwrapBreakOr(action)` | `LoopResult.unwrapBreakOr(action)` |
| `.isBreak()` | `LoopResult.isBreak()` |
| `.isContinue()` | `LoopResult.isContinue()` |

**No postfix for `LoopResult.flatten`** — `.flatten()` already exists on TypedAction for arrays and can't be overloaded with a `this` constraint. Use `LoopResult.flatten()` namespace function only (if added).

## Combinators NOT included

- **`andThen`** — monadic bind on Break ("if breaking, run action that might continue instead") is unintuitive for loop control flow.
- **`or`** / **`and`** — don't map to meaningful loop operations.
- **`flatten`** — `LoopResult<LoopResult<TC, TB>, TB> → LoopResult<TC, TB>` is contrived. Skip unless a use case emerges.
- **`inspect`** — rejected for Option and Result. Same here.
- **`collect`** — collecting LoopResults isn't a pattern. Unlike Option.collect (filter Nones) and Result.collect (short-circuit on Err), there's no natural array-level semantics for LoopResult.

## The `loop` function

Already exists. Takes a body that produces `LoopResult<In, Out>` and extracts the Break value:

```ts
function loop<In, TOut extends LoopResult<any, any>, R extends string = never>(
  body: Pipeable<In, TOut, R>,
): TypedAction<In, ExtractBreakValue<TOut>, R>
```

### Closure form (from LOOP_WITH_CLOSURE.md)

The closure form provides scoped `recur`/`done` pre-bound to the loop's type parameters, avoiding the inference problem where `TContinue` must be specified manually:

```ts
loop(({ recur, done }) =>
  pipe(
    typeCheck,
    classifyErrors,
    branch({
      HasErrors: pipe(forEach(fix), drop(), recur()),
      Clean: done(),
    }),
  ),
)
```

The closure is called at AST construction time. The AST shape is identical — `recur()` and `done()` produce the same Tag nodes. The difference is purely type-level.

## scope / exit (proposed)

`scope` is the generalization of `loop`. Where `loop` repeats on Continue and exits on Break, `scope` just exits on Exit and continues the pipeline otherwise.

### ScopeResult

```ts
type ScopeResultDef<TExit, TContinue> = { Exit: TExit; Continue: TContinue };
type ScopeResult<TExit, TContinue> = TaggedUnion<ScopeResultDef<TExit, TContinue>>;
```

### The `?` operator

```ts
scope(({ exit }) =>
  pipe(
    tryAction(step1),
    branch({ Ok: identity(), Err: exit() }),  // ? operator
    tryAction(step2),
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

Whether to implement `loop` on top of `scope` or keep them as independent AST nodes is an implementation question.

### Scope AST node

```rust
pub struct ScopeAction {
    pub body: Box<Action>,
    pub scope_id: ScopeId,
}

pub struct ExitAction {
    pub scope_id: ScopeId,
}
```

The `scope_id` connects `exit()` to its enclosing `scope`. The engine unwinds frames back to the Scope frame when it sees an Exit tag.

### Sugar: `propagate`

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

## Files to change

### For LoopResult namespace

| File | What changes |
|------|-------------|
| `libs/barnum/src/builtins.ts` | Add `LoopResult` namespace object with recur, done, mapBreak, mapContinue, unwrapBreakOr, isBreak, isContinue. Top-level `recur()`/`done()` stay as exports. |
| `libs/barnum/src/ast.ts` | Add postfix method signatures to TypedAction interface. Add method implementations in `typedAction()`. |
| `libs/barnum/tests/types.test.ts` | Type-level tests for namespace combinators and postfix methods. |
| `libs/barnum/tests/patterns.test.ts` | AST shape tests for namespace combinators. |

### For scope/exit (future)

| File | What changes |
|------|-------------|
| `libs/barnum/src/ast.ts` | Add `ScopeAction` to `Action` union. Add `scope()` combinator. |
| `libs/barnum/src/builtins.ts` | Add `exit()` constructor (or provide via scope closure). |
| Rust AST | Add `Action::Scope` variant. Add `ScopeId` newtype. |
| Rust engine | Add `Frame::Scope` variant. Handle Exit tag unwinding. |
