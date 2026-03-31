# Result<TValue, TError>

## The type

```ts
type ResultDef<TValue, TError> = { Ok: TValue; Err: TError };
type Result<TValue, TError> = TaggedUnion<ResultDef<TValue, TError>>;
```

Same pattern as `Option<T>` — a `TaggedUnion` carrying `__def` so `.branch()` works via `ExtractDef`.

## Constructors

### `Result.ok`

```ts
Result.ok<TValue, TError>(): TypedAction<TValue, Result<TValue, TError>>
// = tag<ResultDef<TValue, TError>, "Ok">("Ok")
```

### `Result.err`

```ts
Result.err<TValue, TError>(): TypedAction<TError, Result<TValue, TError>>
// = tag<ResultDef<TValue, TError>, "Err">("Err")
```

## Transforming

### `Result.map` — transform the Ok value

```ts
Result.map<TValue, TOut, TError>(
  action: Pipeable<TValue, TOut>,
): TypedAction<Result<TValue, TError>, Result<TOut, TError>>
```

Desugars to:
```ts
branch({
  Ok: pipe(action, Result.ok<TOut, TError>()),
  Err: Result.err<TOut, TError>(),
})
```

### `Result.mapErr` — transform the Err value

```ts
Result.mapErr<TValue, TError, TErrorOut>(
  action: Pipeable<TError, TErrorOut>,
): TypedAction<Result<TValue, TError>, Result<TValue, TErrorOut>>
```

Desugars to:
```ts
branch({
  Ok: Result.ok<TValue, TErrorOut>(),
  Err: pipe(action, Result.err<TValue, TErrorOut>()),
})
```

## Chaining

### `Result.andThen` — monadic bind (flatMap) for Ok

```ts
Result.andThen<TValue, TOut, TError>(
  action: Pipeable<TValue, Result<TOut, TError>>,
): TypedAction<Result<TValue, TError>, Result<TOut, TError>>
```

If Ok, pass value to `action` (which returns a new Result). If Err, propagate.

Desugars to:
```ts
branch({
  Ok: action,
  Err: Result.err<TOut, TError>(),
})
```

### `Result.or` — fallback on Err

```ts
Result.or<TValue, TError, TErrorOut>(
  fallback: Pipeable<TError, Result<TValue, TErrorOut>>,
): TypedAction<Result<TValue, TError>, Result<TValue, TErrorOut>>
```

If Ok, keep it. If Err, pass error to `fallback` (which returns a new Result).

Desugars to:
```ts
branch({
  Ok: Result.ok<TValue, TErrorOut>(),
  Err: fallback,
})
```

### `Result.and` — replace Ok value with another Result

```ts
Result.and<TValue, TOut, TError>(
  other: Pipeable<never, Result<TOut, TError>>,
): TypedAction<Result<TValue, TError>, Result<TOut, TError>>
```

If Ok, discard value and return `other`. If Err, propagate.

Desugars to:
```ts
branch({
  Ok: pipe(drop(), other),
  Err: Result.err<TOut, TError>(),
})
```

## Extracting

### `Result.unwrapOr` — extract Ok or compute default from Err

```ts
Result.unwrapOr<TValue, TError>(
  defaultAction: Pipeable<TError, TValue>,
): TypedAction<Result<TValue, TError>, TValue>
```

Takes an action that receives the Err payload and produces a fallback value.

Desugars to:
```ts
branch({
  Ok: identity(),
  Err: defaultAction,
})
```

## Flattening

### `Result.flatten` — unwrap nested Result

```ts
Result.flatten<TValue, TError>(): TypedAction<
  Result<Result<TValue, TError>, TError>,
  Result<TValue, TError>
>
```

Desugars to:
```ts
branch({
  Ok: identity(),
  Err: Result.err<TValue, TError>(),
})
```

## Postfix methods on TypedAction

Gated by `this` parameter constraints (same pattern as `.mapOption()`). Names include `Result` where they'd collide with Option or existing methods.

**Naming convention**: unique names (`.isOk()`, `.ok()`) stand alone. Shared names (`.map`, `.andThen`, `.unwrapOr`) get a `Result` suffix to avoid collision with Option postfix methods.

| Postfix | Equivalent to |
|---|---|
| `.mapResult(action)` | `Result.map(action)` |
| `.mapErrResult(action)` | `Result.mapErr(action)` |
| `.andThenResult(action)` | `Result.andThen(action)` |
| `.orResult(fallback)` | `Result.or(fallback)` |
| `.andResult(other)` | `Result.and(other)` |
| `.unwrapOrResult(action)` | `Result.unwrapOr(action)` |
| `.ok()` | `Result.toOption()` — Ok → Some, Err → None |
| `.err()` | `Result.toOptionErr()` — Err → Some, Ok → None |
| `.transposeResult()` | `Result.transpose()` — Result\<Option\<T\>, E\> → Option\<Result\<T, E\>\> |
| `.isOk()` | `Result.isOk()` |
| `.isErr()` | `Result.isErr()` |

**No postfix for `Result.flatten`** — `.flatten()` already exists on TypedAction for arrays and can't be overloaded with a `this` constraint (the method is defined unconditionally on all TypedActions). Use `Result.flatten()` as a namespace function only.

## Missing Option postfix methods

Same naming convention. Option already has `.mapOption()`. These are missing:

| Postfix | Equivalent to |
|---|---|
| `.andThenOption(action)` | `Option.andThen(action)` |
| `.unwrapOrOption(action)` | `Option.unwrapOr(action)` |
| `.filterOption(predicate)` | `Option.filter(predicate)` |
| `.isSome()` | `Option.isSome()` |
| `.isNone()` | `Option.isNone()` |

**No postfix for `Option.flatten`** — same reason as Result. Use `Option.flatten()` namespace function only.

## Collection

### `Result.collect` — short-circuit on first Err

```ts
Result.collect<TValue, TError>(): TypedAction<Result<TValue, TError>[], Result<TValue[], TError>>
```

If all elements are Ok, collect values into `Ok(TValue[])`. On first Err, short-circuit with that error.

Implemented as a builtin handler (`CollectResult`) — same pattern as `CollectSome` for Option.

## Files to change

| File | What changes |
|------|-------------|
| `libs/barnum/src/ast.ts` | Add `ResultDef<TValue, TError>`, `Result<TValue, TError>` type aliases |
| `libs/barnum/src/builtins.ts` | Add `Result` namespace with all combinators |
| `libs/barnum/tests/types.test.ts` | Type-level tests |
| `libs/barnum/tests/patterns.test.ts` | AST shape tests |
