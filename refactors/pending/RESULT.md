# Result types: comprehensive combinator library

> **Convention**: All discriminated unions use `TaggedUnion<Def>` — every variant carries `{ kind: K; value: T; __def?: Def }`. This is not optional: **all union variants must carry `__def`**, no exceptions. All union constructors require the full variant map as a type parameter so the output type carries `__def`. Branch uses `ExtractDef` for inference and auto-unwraps `value` before each case handler.

> **Status**: Design doc only. Not implementing yet — needs more thought on error handling strategy, interaction with scope/exit, and whether `tryAction` is the right primitive.

## The type

```ts
type ResultDef<T, E> = {
  Ok: T;
  Err: E;
};

type Result<T, E> = TaggedUnion<ResultDef<T, E>>;
// = { kind: "Ok"; value: T; __def?: ResultDef<T, E> }
// | { kind: "Err"; value: E; __def?: ResultDef<T, E> }
```

## Where Results come from

In Rust, any function can return `Result`. In barnum, handlers cross a serialization boundary — the handler runs in a subprocess and its output is JSON. Two potential sources:

### 1. Explicit handler return

The handler explicitly returns `Result<T, E>`:

```ts
// Handler code:
export default async function callApi(input: ApiRequest): Promise<Result<ApiResponse, ApiError>> {
  try {
    const response = await fetch(input.url);
    return { kind: "Ok", value: await response.json() };
  } catch (e) {
    return { kind: "Err", value: { message: e.message } };
  }
}
```

This works today — it's just a tagged union. The handler returns `{ kind, value }` and `branch` dispatches on it.

### 2. `tryAction` — catch handler failures

A wrapper that catches handler runtime errors and wraps them as `Err`:

```ts
function tryAction<In, Out, E = unknown>(
  action: TypedAction<In, Out>,
): TypedAction<In, Result<Out, E>>
```

If the action succeeds, wraps output as `Ok`. If the handler throws/panics, wraps the error as `Err`.

This is a runtime/engine concern — the Rust scheduler needs to catch handler failures and produce `Err` variants instead of propagating the error. Currently, handler failures are unrecoverable. `tryAction` makes them first-class.

**Open question**: What type is `E`? In Rust, errors are typed. In barnum, a handler failure could be anything — a JSON parse error, a timeout, a panic. Probably `E = { message: string; [key: string]: unknown }` or similar. Or leave it generic and let the engine decide.

## API surface: the `Result` namespace

Same pattern as `Option`. All combinators live on `Result.`.

```ts
import { Result } from "@barnum/barnum";

pipe(
  callApi,
  Result.map(extractData),
  Result.unwrapOr(constant(fallbackData)),
)
```

### Design principle: actions, not values

Same as Option — Rust's eager/lazy pairs collapse:

| Rust has two | Barnum has one | Why |
|---|---|---|
| `unwrap_or(val)` / `unwrap_or_else(f)` | `Result.unwrapOr(action)` | Actions are already lazy |
| `or(res)` / `or_else(f)` | `Result.or(action)` | Actions are already lazy |
| `map_or(val, f)` / `map_or_else(d, f)` | `Result.mapOr(defaultAction, action)` | Actions are already lazy |

## Constructors

### `Result.ok` / `Result.err`

```ts
Result.ok<T, E>(): TypedAction<T, Result<T, E>>
// = tag<ResultDef<T, E>, "Ok">("Ok")

Result.err<T, E>(): TypedAction<E, Result<T, E>>
// = tag<ResultDef<T, E>, "Err">("Err")
```

Both carry the full `ResultDef<T, E>` so `__def` is populated.

## Transforming

### `Result.map` — transform the Ok value

```ts
Result.map<T, U, E>(action: TypedAction<T, U>): TypedAction<Result<T, E>, Result<U, E>>
```

Desugars to:
```ts
branch({
  Ok: pipe(action, Result.ok<U, E>()),
  Err: Result.err<U, E>(),
})
```

### `Result.mapErr` — transform the Err value

```ts
Result.mapErr<T, E, F>(action: TypedAction<E, F>): TypedAction<Result<T, E>, Result<T, F>>
```

Desugars to:
```ts
branch({
  Ok: Result.ok<T, F>(),
  Err: pipe(action, Result.err<T, F>()),
})
```

### `Result.inspect` — side effect on Ok

```ts
Result.inspect<T, E>(action: TypedAction<T, unknown>): TypedAction<Result<T, E>, Result<T, E>>
```

Desugars to:
```ts
branch({
  Ok: pipe(tap(action), Result.ok<T, E>()),
  Err: Result.err<T, E>(),
})
```

### `Result.inspectErr` — side effect on Err

```ts
Result.inspectErr<T, E>(action: TypedAction<E, unknown>): TypedAction<Result<T, E>, Result<T, E>>
```

Desugars to:
```ts
branch({
  Ok: Result.ok<T, E>(),
  Err: pipe(tap(action), Result.err<T, E>()),
})
```

### `Result.mapOr` — transform Ok or provide default

```ts
Result.mapOr<T, U, E>(
  defaultAction: TypedAction<E, U>,
  action: TypedAction<T, U>,
): TypedAction<Result<T, E>, U>
```

Desugars to:
```ts
branch({
  Ok: action,
  Err: defaultAction,  // receives E, produces U
})
```

Note: unlike Rust's `map_or` where the default is a value, the default here receives the `Err` payload. This is more powerful — the default can inspect the error.

## Boolean operations (and/or)

### `Result.and` — return other if Ok, Err otherwise

```ts
Result.and<T, U, E>(other: TypedAction<void, Result<U, E>>): TypedAction<Result<T, E>, Result<U, E>>
```

Desugars to:
```ts
branch({
  Ok: pipe(drop(), other),
  Err: Result.err<U, E>(),
})
```

### `Result.andThen` (flatMap) — chain result-producing actions

```ts
Result.andThen<T, U, E>(action: TypedAction<T, Result<U, E>>): TypedAction<Result<T, E>, Result<U, E>>
```

The monadic bind for Result. If `Ok`, pass value to `action`. If `Err`, propagate.

Desugars to:
```ts
branch({
  Ok: action,       // receives T, produces Result<U, E>
  Err: Result.err<U, E>(),  // propagate error
})
```

### `Result.or` — fallback if Err

```ts
Result.or<T, E, F>(fallback: TypedAction<E, Result<T, F>>): TypedAction<Result<T, E>, Result<T, F>>
```

If `Ok`, keep it. If `Err`, try `fallback` (which receives the error payload).

Desugars to:
```ts
branch({
  Ok: Result.ok<T, F>(),
  Err: fallback,  // receives E, produces Result<T, F>
})
```

## Extracting values

### `Result.unwrap` — extract Ok or panic

```ts
Result.unwrap<T, E>(): TypedAction<Result<T, E>, T>
```

Blocked on error handling primitives. Same as `Option.unwrap`.

### `Result.unwrapErr` — extract Err or panic

```ts
Result.unwrapErr<T, E>(): TypedAction<Result<T, E>, E>
```

Also blocked on error handling.

### `Result.expect` / `Result.expectErr` — extract or panic with message

```ts
Result.expect<T, E>(message: string): TypedAction<Result<T, E>, T>
Result.expectErr<T, E>(message: string): TypedAction<Result<T, E>, E>
```

Blocked on error handling.

### `Result.unwrapOr` — extract Ok or default from Err

```ts
Result.unwrapOr<T, E>(defaultAction: TypedAction<E, T>): TypedAction<Result<T, E>, T>
```

Takes an action that receives the `Err` payload. More powerful than Rust's `unwrap_or` (which ignores the error).

Desugars to:
```ts
branch({
  Ok: identity(),
  Err: defaultAction,  // receives E, produces T
})
```

## Querying

### `Result.isOk` / `Result.isErr`

```ts
Result.isOk<T, E>(): TypedAction<Result<T, E>, boolean>
Result.isErr<T, E>(): TypedAction<Result<T, E>, boolean>
```

Desugar to:
```ts
branch({ Ok: pipe(drop(), constant(true)), Err: pipe(drop(), constant(false)) })
```

Rarely useful — branch directly on `Ok`/`Err` instead. Present for completeness.

### `Result.isOkAnd` / `Result.isErrAnd`

```ts
Result.isOkAnd<T, E>(predicate: TypedAction<T, boolean>): TypedAction<Result<T, E>, boolean>
Result.isErrAnd<T, E>(predicate: TypedAction<E, boolean>): TypedAction<Result<T, E>, boolean>
```

Same caveat as Option — boolean output limits composability.

## Conversions to Option

### `Result.ok` (as converter) — Result<T, E> → Option<T>

```ts
Result.toOption<T, E>(): TypedAction<Result<T, E>, Option<T>>
```

Desugars to:
```ts
branch({
  Ok: Option.some<T>(),
  Err: pipe(drop(), Option.none<T>()),
})
```

### `Result.err` (as converter) — Result<T, E> → Option<E>

```ts
Result.toOptionErr<T, E>(): TypedAction<Result<T, E>, Option<E>>
```

Desugars to:
```ts
branch({
  Ok: pipe(drop(), Option.none<E>()),
  Err: Option.some<E>(),
})
```

### `Result.transpose` — Result<Option<T>, E> → Option<Result<T, E>>

```ts
Result.transpose<T, E>(): TypedAction<Result<Option<T>, E>, Option<Result<T, E>>>
```

Desugars to:
```ts
branch({
  Ok: branch({                                   // receives Option<T>
    Some: pipe(Result.ok<T, E>(), Option.some()), // T → Result<T, E> → Option<Result<T, E>>
    None: pipe(drop(), Option.none()),             // void → Option<Result<T, E>>
  }),
  Err: pipe(Result.err<T, E>(), Option.some()),   // E → Result<T, E> → Option<Result<T, E>>
})
```

## Flattening

### `Result.flatten` — Result<Result<T, E>, E> → Result<T, E>

```ts
Result.flatten<T, E>(): TypedAction<Result<Result<T, E>, E>, Result<T, E>>
```

Desugars to:
```ts
Result.andThen<Result<T, E>, T, E>(identity())
// = branch({ Ok: identity(), Err: Result.err() })
```

## The `?` operator — scope + exit

The most important Result pattern. See CONTROL_FLOW.md for `scope`/`exit` design.

```ts
scope(({ exit }) =>
  pipe(
    tryAction(step1),
    branch({ Ok: identity(), Err: exit() }),  // ?
    tryAction(step2),
    branch({ Ok: identity(), Err: exit() }),  // ?
    tryAction(step3),
    branch({ Ok: identity(), Err: exit() }),  // ?
  ),
)
```

Sugar: `propagate(exit)` = `branch({ Ok: identity(), Err: exit() })`.

Higher-level: `tryScope(pipe(step1, step2, step3))`.

## Collection combinators

### `Result.collect` — Result<T, E>[] → Result<T[], E>

```ts
Result.collect<T, E>(): TypedAction<Result<T, E>[], Result<T[], E>>
```

If all elements are `Ok`, collect values into `Ok(T[])`. On first `Err`, short-circuit with that error.

This is Rust's `Iterator::collect::<Result<Vec<T>, E>>()`. Implemented as a builtin handler (`CollectResult`) — same pattern as `CollectSome`.

### `Result.partition` — Result<T, E>[] → { ok: T[]; err: E[] }

```ts
Result.partition<T, E>(): TypedAction<Result<T, E>[], { ok: T[]; err: E[] }>
```

Split into successes and failures. More useful than `collect` when you want to process both.

### `Result.collectOk` — Result<T, E>[] → T[]

```ts
Result.collectOk<T, E>(): TypedAction<Result<T, E>[], T[]>
```

Drop errors, unwrap successes. Equivalent to `Option.collect` but for Results.

## Combinators NOT ported from Rust

### `unwrapOrDefault`

No traits, no defaults. `Result.unwrapOr(constant(defaultValue))` is the equivalent.

### Mutation: `as_ref`, `as_mut`, etc.

Not applicable — immutable AST nodes.

## Priority

### Tier 1: core

- `Result.ok()` / `Result.err()` — constructors
- `Result.map(action)` — transform Ok value
- `Result.mapErr(action)` — transform Err value
- `Result.andThen(action)` — monadic bind / flatMap for Ok
- `Result.unwrapOr(action)` — extract Ok with fallback

### Tier 2: useful

- `Result.or(fallback)` — fallback on Err
- `Result.mapOr(default, action)` — transform Ok or default from Err
- `Result.flatten()` — unwrap nested Result
- `Result.toOption()` — convert to Option
- `propagate(exit)` — sugar for ? operator

### Tier 3: nice to have

- `Result.and(other)` — discard Ok value, use other
- `Result.inspect(action)` / `Result.inspectErr(action)` — side effects
- `Result.collect()` / `Result.partition()` / `Result.collectOk()` — collection ops
- `Result.transpose()` — swap Result/Option nesting
- `Result.toOptionErr()` — convert error side to Option

### Deferred / blocked

- `Result.unwrap()` / `Result.expect(msg)` — needs error handling primitives
- `Result.unwrapErr()` / `Result.expectErr(msg)` — same
- `tryAction(handler)` — needs engine support for catching handler failures
- `tryScope` — needs scope/exit from CONTROL_FLOW.md

## Open questions

### Error type for `tryAction`

What type is `E` when a handler fails? Options:

1. **Structured error**: `{ message: string; handler: string; module: string }` — typed, the engine constructs it
2. **`unknown`** — punt on the type, let the user branch/inspect
3. **Generic with engine config** — the engine's error format is configurable per deployment

Leaning toward option 1 — a well-known error shape that the engine always produces.

### `tryAction` vs explicit Result returns

Should handlers return `Result` explicitly, or should the engine catch failures and wrap them? Both have value:

- **Explicit return**: Handler decides what's an error. Full type safety. Handler can return partial results on failure.
- **`tryAction` wrapping**: Catches unexpected panics/timeouts. Handler code is simpler (just throw). But error type is engine-defined, not domain-specific.

Probably both. Explicit Result returns for domain errors. `tryAction` for infrastructure failures.

### Interaction with scope/exit

The `?` operator (scope + exit) works on any tagged union, not just Result. But the ergonomic sugar (`propagate`, `tryScope`) is Result-specific. Should the sugar be generic?

```ts
// Generic:
propagate(exit, "Ok", "Err")  // specify which variant continues and which exits

// Result-specific:
propagate(exit)  // hardcoded to Ok/Err
```

Result-specific is cleaner for the common case. Generic can be added later if needed.

## Files to change (when implementing)

| File | What changes |
|------|-------------|
| `libs/barnum/src/ast.ts` | Add `ResultDef<T, E>`, `Result<T, E>` type aliases. |
| `libs/barnum/src/builtins.ts` | Add `Result` namespace object with all combinators. |
| `libs/barnum_ast/src/lib.rs` | If adding `tryAction`: new `Action::Try` variant. |
| `libs/barnum_engine/src/workflow_state.rs` | If adding `tryAction`: catch handler failures, produce Err variant. |
| `libs/barnum/tests/types.test.ts` | Type-level tests. |
| `libs/barnum/tests/patterns.test.ts` | Runtime AST shape tests. |
