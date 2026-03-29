# Option types: comprehensive combinator library

> **Convention**: All discriminated unions use `TaggedUnion<Def>` — every variant carries `{ kind: K; value: T; __def?: Def }`. This is not optional: **all union variants must carry `__def`**, no exceptions. Constructors like `some()`, `none()`, `recur()`, `done()`, and `tag()` all require the full variant map as a type parameter so the output type carries `__def`. Branch uses `ExtractDef` for inference and auto-unwraps `value` before each case handler.

## The type

```ts
type OptionDef<T> = {
  Some: T;
  None: void;
};

type Option<T> = TaggedUnion<OptionDef<T>>;
// = { kind: "Some"; value: T; __def?: OptionDef<T> }
// | { kind: "None"; value: void; __def?: OptionDef<T> }
```

## API surface: the `Option` namespace

All combinators live on an `Option` namespace object. This is the preferred API — no separate imports, everything discoverable via `Option.`.

```ts
import { Option } from "@barnum/barnum";

pipe(
  lookupUser,
  Option.map(normalize),
  Option.unwrapOr(constant(defaultUser)),
)
```

### Design principle: actions, not values

In barnum, all "arguments" to combinators are **actions** (AST nodes), not runtime values. This means Rust's paired methods that differ only in eagerness collapse into one:

| Rust has two | Barnum has one | Why |
|---|---|---|
| `unwrap_or(val)` / `unwrap_or_else(f)` | `Option.unwrapOr(action)` | Actions are already lazy |
| `or(opt)` / `or_else(f)` | `Option.or(action)` | Actions are already lazy |
| `map_or(val, f)` / `map_or_else(d, f)` | `Option.mapOr(defaultAction, action)` | Actions are already lazy |
| `ok_or(err)` / `ok_or_else(f)` | `Option.okOr(action)` | Actions are already lazy |

## Constructors

### `Option.some` / `Option.none`

Both carry the full `OptionDef<T>` so the output includes `__def`:

```ts
Option.some<T>(): TypedAction<T, Option<T>>
// = tag<OptionDef<T>, "Some">("Some")
// Input: T, Output: Option<T> (full union with __def)

Option.none<T>(): TypedAction<void, Option<T>>
// = tag<OptionDef<T>, "None">("None")
// Input: void, Output: Option<T> (full union with __def)
```

## Extracting values

### `Option.unwrap` — extract or panic

```ts
Option.unwrap<T>(): TypedAction<Option<T>, T>
```

Requires error handling (scope/exit or Result). Without it, None is a runtime error.

Desugars to:
```ts
branch({
  Some: identity(),  // receives T
  None: panic("called unwrap on None"),  // TBD: needs error primitive
})
```

**Status**: Blocked on error handling primitives. Note for completeness.

### `Option.expect` — extract or panic with message

```ts
Option.expect<T>(message: string): TypedAction<Option<T>, T>
```

Same as `unwrap` but with a custom error message. Blocked on error handling.

### `Option.unwrapOr` — extract or default

```ts
Option.unwrapOr<T>(defaultAction: TypedAction<void, T>): TypedAction<Option<T>, T>
```

Takes an **action**, not a raw value. Use `Option.unwrapOr(constant("anonymous"))`.

Desugars to:
```ts
branch({
  Some: identity(),       // receives T
  None: defaultAction,    // receives void, produces T
})
```

## Transforming

### `Option.map` — transform the Some value

```ts
Option.map<T, U>(action: TypedAction<T, U>): TypedAction<Option<T>, Option<U>>
```

Apply `action` to the `Some` value, rewrap as `Some`. Pass `None` through unchanged.

Desugars to:
```ts
branch({
  Some: pipe(action, some<U>()),  // receives T, produces Option<U>
  None: none<U>(),                // receives void, produces Option<U>
})
```

### `Option.inspect` — side effect on Some, pass through

```ts
Option.inspect<T>(action: TypedAction<T, unknown>): TypedAction<Option<T>, Option<T>>
```

Run `action` on the `Some` value for side effects, discard its output, keep the original `Option<T>`.

Desugars to:
```ts
branch({
  Some: pipe(tap(action), some<T>()),  // receives T, runs action, re-wraps
  None: none<T>(),                     // pass through
})
```

Note: `tap` currently requires `Record<string, unknown>` input. If `T` isn't an object, this needs a variant of tap that works on any type (just parallel + extractIndex instead of parallel + merge).

### `Option.mapOr` — transform Some or provide default

```ts
Option.mapOr<T, U>(
  defaultAction: TypedAction<void, U>,
  action: TypedAction<T, U>,
): TypedAction<Option<T>, U>
```

Collapses Rust's `map_or` and `map_or_else`. Both args are actions.

Desugars to:
```ts
branch({
  Some: action,         // receives T, produces U
  None: defaultAction,  // receives void, produces U
})
```

## Boolean operations (and/or)

### `Option.and` — return other if Some, None otherwise

```ts
Option.and<T, U>(other: TypedAction<void, Option<U>>): TypedAction<Option<T>, Option<U>>
```

If `Some`, discard the value and evaluate `other`. If `None`, produce `None`.

Desugars to:
```ts
branch({
  Some: pipe(drop(), other),  // discard T, run other to get Option<U>
  None: none<U>(),            // produce None (re-typed for Option<U>)
})
```

### `Option.andThen` (flatMap) — chain option-producing actions

```ts
Option.andThen<T, U>(action: TypedAction<T, Option<U>>): TypedAction<Option<T>, Option<U>>
```

The core monadic bind. If `Some`, pass the value to `action` which returns `Option<U>`. If `None`, stay `None`.

Desugars to:
```ts
branch({
  Some: action,     // receives T, produces Option<U>
  None: none<U>(),  // produce None
})
```

This is `flatMap` / Rust's `and_then`. The most important combinator after `map` and `unwrapOr`.

### `Option.or` — fallback if None

```ts
Option.or<T>(fallback: TypedAction<void, Option<T>>): TypedAction<Option<T>, Option<T>>
```

If `Some`, keep it. If `None`, evaluate `fallback`. Collapses Rust's `or` and `or_else`.

Desugars to:
```ts
branch({
  Some: some<T>(),   // receives T, re-wraps as Some
  None: fallback,    // receives void, produces Option<T>
})
```

### `Option.xor` — exclusive or

```ts
Option.xor<T>(other: TypedAction<void, Option<T>>): TypedAction<Option<T>, Option<T>>
```

Returns `Some` if exactly one of `self` and `other` is `Some`. Otherwise `None`.

This is awkward in barnum — you need to evaluate `other` regardless, then dispatch on the 2×2 matrix of (self, other). Requires nested branches or a parallel + custom logic.

**Status**: Low priority. Expressible but ugly. Skip for now.

### `Option.filter` — conditional keep

```ts
Option.filter<T>(predicate: TypedAction<T, Option<T>>): TypedAction<Option<T>, Option<T>>
```

Rust's `filter` takes a `FnOnce(&T) -> bool`, but barnum has no boolean branch primitive. Two options:

**Option A**: Predicate returns `Option<T>` directly. Then `filter` IS `andThen`:
```ts
Option.filter = Option.andThen  // when predicate returns Option<T>
```
The predicate returns `some()` to keep, `none()` to discard. This is clean and composable, but the signature is really just `andThen` by another name.

**Option B**: Add a `BoolBranch` AST node that dispatches on `true`/`false`. Then:
```ts
Option.filter<T>(predicate: TypedAction<T, boolean>): TypedAction<Option<T>, Option<T>>
// branch({ Some: boolBranch(predicate, { true: some(), false: drop().then(none()) }), None: none() })
```

**Recommendation**: Option A. Don't add a new AST node just for booleans. The predicate-returns-Option pattern is natural:

```ts
pipe(
  lookupUser,
  Option.filter(pipe(
    get("role"),
    // "filter" by returning Some if admin, None otherwise
    branch({ Admin: some(), Guest: pipe(drop(), none()) }),
  )),
)
```

If boolean predicates become common, `BoolBranch` can be added later.

## Flattening and zipping

### `Option.flatten` — unwrap nested Option

```ts
Option.flatten<T>(): TypedAction<Option<Option<T>>, Option<T>>
```

Desugars to:
```ts
Option.andThen<Option<T>, T>(identity())
// = branch({ Some: identity(), None: none<T>() })
```

If `Some`, the value is already `Option<T>` — pass it through. If `None`, stay `None`.

### `Option.zip` — combine two options

```ts
Option.zip<T, U>(other: TypedAction<void, Option<U>>): TypedAction<Option<T>, Option<[T, U]>>
```

If both `self` and `other` are `Some`, produce `Some([t, u])`. Otherwise `None`.

Desugars to:
```ts
Option.andThen(t =>
  // t is the unwrapped T value
  // Evaluate other, map its Some value to [t, u]
  pipe(drop(), other, Option.map(pipe(u => parallel(constant(t), constant(u)))))
)
```

This is awkward because barnum actions don't close over runtime values. A clean implementation needs either:
1. A dedicated `Zip` builtin for options, or
2. The `augment` pattern: `branch({ Some: pipe(augment(pipe(drop(), other)), ...), None: ... })`

**Status**: Medium priority. The desugaring is ugly — worth a dedicated builtin if zip is common.

### `Option.unzip` — split Option of tuple

```ts
Option.unzip<T, U>(): TypedAction<Option<[T, U]>, [Option<T>, Option<U>]>
```

Desugars to:
```ts
branch({
  Some: parallel(
    pipe(extractIndex(0), some<T>()),
    pipe(extractIndex(1), some<U>()),
  ),
  None: parallel(
    pipe(drop(), none<T>()),
    pipe(drop(), none<U>()),
  ),
})
```

**Status**: Low priority. Rarely needed.

## Conversions to Result (deferred)

These require the Result type to exist first. Listed for completeness.

### `Option.okOr` — Option<T> → Result<T, E>

```ts
Option.okOr<T, E>(errAction: TypedAction<void, E>): TypedAction<Option<T>, Result<T, E>>
```

Desugars to:
```ts
branch({
  Some: tag<ResultDef<T, E>, "Ok">("Ok"),
  None: pipe(errAction, tag<ResultDef<T, E>, "Err">("Err")),
})
```

### `Option.transpose` — Option<Result<T, E>> → Result<Option<T>, E>

```ts
Option.transpose<T, E>(): TypedAction<Option<Result<T, E>>, Result<Option<T>, E>>
```

**Status**: Deferred to Result implementation.

## Collection combinators (operating on Option arrays)

These operate on `Option<T>[]` — the output of `forEach(action)` where `action` returns `Option<T>`.

### `Option.collect` (collectSome) — Option<T>[] → T[]

```ts
Option.collect<T>(): TypedAction<Option<T>[], T[]>
```

Drop `None` values, unwrap `Some` values. This is Rust's `Iterator::flatten` over options.

Implemented as a **new builtin handler** (`CollectSome`), not a new AST node. Like `Flatten`, `ExtractField`, and `Tag`, it's a pure data transformation that the engine executes inline. No new control flow, no new frames, no AST changes.

```ts
// BuiltinKind — just a new variant:
| { kind: "CollectSome" }

// Engine: takes Option<T>[], returns T[]
// Iterates the array, keeps items where kind === "Some", extracts value field.
```

### `Option.filterMap` — map + collect in one step

```ts
Option.filterMap<TIn, TOut>(
  action: TypedAction<TIn, Option<TOut>>,
): TypedAction<TIn[], TOut[]>
```

For each element, run `action`. Collect `Some` values, discard `None`.

Desugars to existing AST nodes: `forEach(action).then(Option.collect())`. No new AST needed — just composes `ForEach` + `CollectSome` builtin.

### `Option.partition` — split into Somes and Nones

```ts
Option.partition<T>(): TypedAction<Option<T>[], { some: T[]; none: void[] }>
```

Also a builtin handler if needed. Low priority — `collect` covers the common case.

## Querying (boolean predicates)

### `Option.isSome` / `Option.isNone`

```ts
Option.isSome<T>(): TypedAction<Option<T>, boolean>
Option.isNone<T>(): TypedAction<Option<T>, boolean>
```

Desugar to:
```ts
// isSome
branch({ Some: pipe(drop(), constant(true)), None: pipe(drop(), constant(false)) })
// isNone
branch({ Some: pipe(drop(), constant(false)), None: pipe(drop(), constant(true)) })
```

These are straightforward but rarely useful in practice — booleans can't feed into `branch`, so you'd almost always just branch on `Some`/`None` directly instead. Present for completeness.

### `Option.isSomeAnd`

```ts
Option.isSomeAnd<T>(predicate: TypedAction<T, boolean>): TypedAction<Option<T>, boolean>
```

Desugars to:
```ts
branch({ Some: predicate, None: pipe(drop(), constant(false)) })
```

Same caveat — the boolean output limits composability. Use `branch` directly when possible.

## Combinators NOT ported from Rust

### Mutation: `getOrInsert`, `getOrInsertWith`, `take`, `replace`

These mutate the Option in place. Barnum values are immutable AST nodes.

**Skip.** Not applicable.

### `unwrapOrDefault`

Rust's `unwrap_or_default` uses the `Default` trait. Barnum has no traits. `unwrapOr(constant(defaultValue))` is the equivalent.

**Skip.** Subsumed by `unwrapOr`.

## Priority for implementation

### Tier 1: core

- `Option.some()` / `Option.none()` — constructors
- `Option.map(action)` — transform Some value
- `Option.andThen(action)` — monadic bind / flatMap
- `Option.unwrapOr(action)` — extract with default

These four cover 80% of Option usage. Everything else desugars to `branch` anyway.

### Tier 2: useful

- `Option.or(fallback)` — try alternative on None
- `Option.mapOr(default, action)` — transform or default
- `Option.flatten()` — unwrap nested Option
- `Option.collect()` — `Option<T>[] → T[]` (needs new builtin)

### Tier 3: nice to have

- `Option.and(other)` — discard Some value, use other
- `Option.inspect(action)` — side effect without changing value
- `Option.filter(pred)` — conditional keep (= `andThen` with Option-returning pred)
- `Option.filterMap(action)` — forEach + collect
- `Option.zip(other)` — combine two options

### Deferred

- `Option.okOr(action)` — convert to Result (needs Result type)
- `Option.transpose()` — swap Option/Result nesting (needs Result type)
- `Option.unwrap()` / `Option.expect(msg)` — needs error handling primitives
- `Option.xor(other)` — awkward desugaring, rarely needed

## Postfix methods on TypedAction

The highest-value combinators should also be available as postfix methods, gated by `this` parameter constraint so they're only callable when `Out` matches `Option<T>`:

```ts
// On TypedAction:
mapOption<U>(action: TypedAction<T, U>): TypedAction<In, Option<U>, Refs>
andThen<U>(action: TypedAction<T, Option<U>>): TypedAction<In, Option<U>, Refs>
unwrapOr(defaultAction: TypedAction<void, T>): TypedAction<In, T, Refs>
```

The `this` constraint (Phase 2 from POSTFIX_OPERATORS.md) makes these methods invisible unless `Out` is `Option<T>`. The namespace `Option.map(action)` is the standalone form; `.mapOption(action)` is the postfix form.

**Naming**: Postfix methods include "Option" in the name (`mapOption`, not `map`) to avoid collision with hypothetical Result postfix methods (`mapOk`, `mapErr`). The namespace form doesn't need the suffix because `Option.map` is already unambiguous.

## Implementation notes

### The namespace object

```ts
export const Option = {
  some: <T>(): TypedAction<T, Option<T>> => tag<OptionDef<T>, "Some">("Some"),
  none: <T>(): TypedAction<void, Option<T>> => tag<OptionDef<T>, "None">("None"),
  map: <T, U>(action: Pipeable<T, U>): TypedAction<Option<T>, Option<U>> => ...,
  andThen: <T, U>(action: Pipeable<T, Option<U>>): TypedAction<Option<T>, Option<U>> => ...,
  unwrapOr: <T>(defaultAction: Pipeable<void, T>): TypedAction<Option<T>, T> => ...,
  or: <T>(fallback: Pipeable<void, Option<T>>): TypedAction<Option<T>, Option<T>> => ...,
  // ... etc
} as const;
```

Each method is a thin wrapper around `branch` + the appropriate case handlers. No new AST nodes needed (except `collect`).

### Runtime representation

All Option combinators produce standard AST nodes (Branch, Chain, Invoke). The `Option` namespace is a compile-time convenience — it generates the same AST you'd write by hand with `branch`.

The `Option<T>` type itself is just `TaggedUnion<OptionDef<T>>` — same `{ kind, value, __def? }` shape as any other tagged union. No special runtime support.

### `collect` (collectSome) — new builtin handler

`Option.collect()` is a new **builtin handler** (`CollectSome`), same category as `Flatten`, `ExtractField`, `Tag`. Pure data transformation — takes `Option<T>[]`, filters to `Some` variants, extracts values, returns `T[]`. No new AST nodes, no new frames, no new control flow. The engine already executes builtins inline.

`Option.filterMap(action)` composes existing primitives: `forEach(action).then(Option.collect())`. No dedicated AST node needed.

## Files to change

| File | What changes |
|------|-------------|
| `libs/barnum/src/ast.ts` | Add `OptionDef<T>`, `Option<T>` type aliases. Add `CollectSome` to `BuiltinKind`. |
| `libs/barnum/src/builtins.ts` | Add `Option` namespace object with all tier 1–2 combinators. Each is a thin wrapper around `branch` + existing builtins. |
| `libs/barnum/tests/types.test.ts` | Type-level tests for Option combinators: `Option.map` preserves Option wrapper, `Option.andThen` chains correctly, `Option.unwrapOr` extracts, etc. |
| `libs/barnum/tests/patterns.test.ts` | Runtime tests: Option combinators produce correct AST shapes. |

### Existing functions that don't change

- **`tag()`** — `Option.some()` and `Option.none()` call `tag<OptionDef<T>, K>()` internally. No signature change.
- **`branch()`** — Already supports `TaggedUnion` via `ExtractDef`/`BranchKeys`/`BranchPayload`. No change.
- **`identity()`, `drop()`, `pipe()`, `constant()`** — Used inside Option combinators' desugarings. No change.
- **`forEach()`** — Works on arrays. No change. `Option.filterMap` composes `forEach` + `Option.collect`.

### New builtin for `collect`

`Option.collect()` is a new builtin handler, not a new AST node. No changes to the Action enum, no new frames, no new engine control flow.

**TypeScript** (`ast.ts`): Add `| { kind: "CollectSome" }` to `BuiltinKind`.

**Rust AST** (`barnum_ast`): Add `BuiltinKind::CollectSome` variant.

**Rust engine** (`barnum_engine`): Handle `CollectSome` in builtin execution — iterate input array, keep `{ kind: "Some" }` items, extract their `value` fields, return collected array.

Schema files regenerate automatically via pre-commit hook (trivial — new enum variant).
