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

In barnum, all arguments to combinators are **actions** (AST nodes), not runtime values. Rust's paired methods that differ only in eagerness collapse into one:

| Rust has two | Barnum has one | Why |
|---|---|---|
| `unwrap_or(val)` / `unwrap_or_else(f)` | `Option.unwrapOr(action)` | Actions are already lazy |
| `or(opt)` / `or_else(f)` | `Option.or(action)` | Actions are already lazy |
| `ok_or(err)` / `ok_or_else(f)` | `Option.okOr(action)` | Actions are already lazy |

### Postfix support

Most Option combinators take `Option<T>` as input and CAN be postfix methods on TypedAction, gated by `this` constraint when `Out` is `Option<T>`:

```ts
// Prefix (namespace):
pipe(lookup, Option.map(normalize))

// Postfix (method on TypedAction):
lookup.mapOption(normalize)
```

The exception is collection-level combinators (`collect`, `filterMap`, `partition`) — they take `Option<T>[]`, and gating a `this` constraint on `Out extends Option<infer T>[]` isn't feasible in TypeScript. These are prefix-only, used via `.then()`:

```ts
forEach(action).then(Option.collect())
```

Postfix naming includes "Option" to avoid collision with Result methods: `.mapOption()`, `.andThenOption()`, `.unwrapOr()`, `.optionOr()`.

---

Combinators below are ordered from most fundamental to least fundamental. Everything desugars to `branch` + existing builtins.

## 1. Constructors: `Option.some` / `Option.none`

```ts
Option.some<T>(): TypedAction<T, Option<T>>
// = tag<OptionDef<T>, "Some">("Some")

Option.none<T>(): TypedAction<void, Option<T>>
// = tag<OptionDef<T>, "None">("None")
```

Both carry the full `OptionDef<T>` so `__def` is populated.

## 2. `Option.andThen` — monadic bind (flatMap)

```ts
Option.andThen<T, U>(action: TypedAction<T, Option<U>>): TypedAction<Option<T>, Option<U>>
```

The most fundamental combinator. If `Some`, pass the value to `action` which returns `Option<U>`. If `None`, stay `None`. Everything else can be derived from `andThen` + constructors.

Desugars to:
```ts
branch({
  Some: action,     // receives T, produces Option<U>
  None: none<U>(),  // produce None
})
```

This is Rust's `and_then` / Haskell's `>>=`.

## 3. `Option.map` — transform the Some value

```ts
Option.map<T, U>(action: TypedAction<T, U>): TypedAction<Option<T>, Option<U>>
```

Apply `action` to the `Some` value, rewrap as `Some`. Pass `None` through unchanged. Derivable from `andThen`: `Option.andThen(pipe(action, some()))`.

Desugars to:
```ts
branch({
  Some: pipe(action, some<U>()),  // receives T, produces Option<U>
  None: none<U>(),                // receives void, produces Option<U>
})
```

## 4. `Option.unwrapOr` — extract or default

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

## 5. `Option.or` — fallback if None

```ts
Option.or<T>(fallback: TypedAction<void, Option<T>>): TypedAction<Option<T>, Option<T>>
```

If `Some`, keep it. If `None`, evaluate `fallback`.

Desugars to:
```ts
branch({
  Some: some<T>(),   // receives T, re-wraps as Some
  None: fallback,    // receives void, produces Option<T>
})
```

## 6. `Option.and` — discard Some, use other

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

## 7. `Option.flatten` — unwrap nested Option

```ts
Option.flatten<T>(): TypedAction<Option<Option<T>>, Option<T>>
```

Derivable: `Option.andThen(identity())`.

Desugars to:
```ts
branch({
  Some: identity(),  // receives Option<T>, passes through
  None: none<T>(),
})
```

## 8. `Option.filter` — conditional keep

```ts
Option.filter<T>(predicate: TypedAction<T, Option<T>>): TypedAction<Option<T>, Option<T>>
```

Predicate returns `Option<T>` (not `boolean` — booleans can't feed into branch). Returns `some()` to keep, `none()` to discard. This IS `andThen` — same signature, same desugaring. Provided as an alias for readability when the intent is filtering rather than chaining.

## 9. `Option.inspect` — side effect on Some

```ts
Option.inspect<T>(action: TypedAction<T, unknown>): TypedAction<Option<T>, Option<T>>
```

Run `action` on the `Some` value for side effects, discard its output, keep the original `Option<T>`.

Desugars to:
```ts
branch({
  Some: pipe(tap(action), some<T>()),  // receives T, runs action, re-wraps
  None: none<T>(),
})
```

Note: `tap` currently requires `Record<string, unknown>` input. If `T` isn't an object, needs a `tap` variant that works on any type.

## 10. Collection combinators (prefix-only)

These operate on `Option<T>[]`. Cannot be postfix — see note above.

### `Option.collect` — Option<T>[] → T[]

```ts
Option.collect<T>(): TypedAction<Option<T>[], T[]>
```

Drop `None` values, unwrap `Some` values. New **builtin handler** (`CollectSome`), same category as `Flatten`/`ExtractField`/`Tag`. Pure data transformation, no AST changes.

### `Option.filterMap` — map + collect

```ts
Option.filterMap<TIn, TOut>(
  action: TypedAction<TIn, Option<TOut>>,
): TypedAction<TIn[], TOut[]>
```

Desugars to: `forEach(action).then(Option.collect())`.

### `Option.partition` — split Somes and Nones

```ts
Option.partition<T>(): TypedAction<Option<T>[], { some: T[]; none: void[] }>
```

Builtin handler if needed. Low priority.

## 11. Querying (boolean predicates)

Rarely useful — you'd branch on `Some`/`None` directly. Present for completeness.

### `Option.isSome` / `Option.isNone`

```ts
Option.isSome<T>(): TypedAction<Option<T>, boolean>
Option.isNone<T>(): TypedAction<Option<T>, boolean>
```

### `Option.isSomeAnd`

```ts
Option.isSomeAnd<T>(predicate: TypedAction<T, boolean>): TypedAction<Option<T>, boolean>
```

Desugars to:
```ts
branch({ Some: predicate, None: pipe(drop(), constant(false)) })
```

## Deferred

### Blocked on error handling

- `Option.unwrap()` — extract or panic on None
- `Option.expect(msg)` — extract or panic with message

### Blocked on Result type

- `Option.okOr(errAction)` — `Option<T> → Result<T, E>`
- `Option.transpose()` — `Option<Result<T, E>> → Result<Option<T>, E>`

### Low priority

- `Option.xor(other)` — awkward desugaring (2×2 matrix), rarely needed
- `Option.zip(other)` — can't close over runtime values, needs dedicated builtin
- `Option.unzip()` — rarely needed

## Combinators NOT ported from Rust

- **Mutation** (`getOrInsert`, `take`, `replace`): immutable AST, not applicable.
- **`unwrapOrDefault`**: no traits. `unwrapOr(constant(defaultValue))` is the equivalent.

## Files to change

| File | What changes |
|------|-------------|
| `libs/barnum/src/ast.ts` | Add `OptionDef<T>`, `Option<T>` type aliases. Add `CollectSome` to `BuiltinKind`. |
| `libs/barnum/src/builtins.ts` | Add `Option` namespace object with all combinators. Each is a thin wrapper around `branch` + existing builtins. |
| `libs/barnum/tests/types.test.ts` | Type-level tests for Option combinators. |
| `libs/barnum/tests/patterns.test.ts` | Runtime tests: correct AST shapes. |

### Existing functions that don't change

- **`tag()`** — `Option.some()`/`Option.none()` call `tag<OptionDef<T>, K>()` internally.
- **`branch()`** — Already supports `TaggedUnion` via `ExtractDef`.
- **`identity()`, `drop()`, `pipe()`, `constant()`** — Used inside desugarings.
- **`forEach()`** — `Option.filterMap` composes `forEach` + `Option.collect`.

### New builtin for `collect`

New builtin handler, not AST node. No changes to Action enum.

**TypeScript** (`ast.ts`): Add `| { kind: "CollectSome" }` to `BuiltinKind`.

When a Rust AST/engine exists: add `BuiltinKind::CollectSome` variant. Iterate input array, keep `{ kind: "Some" }` items, extract `value` fields, return collected array.
