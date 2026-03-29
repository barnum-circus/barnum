# Option types: representing optional values in barnum pipelines

## Problem

Many real workflows need to express "this step might not produce a value." Examples:

- `analyze` might find no refactoring opportunities for a file
- `typeCheck` might find no errors
- A lookup handler might not find a matching record
- A validation step might accept or reject input

Currently, handlers return arrays (empty = no results) or discriminated unions (`{ kind: "Found", value } | { kind: "NotFound" }`). There's no standardized option type or combinators that operate on it.

## Proposed option type

```ts
type Option<T> =
  | { kind: "Some"; value: T }
  | { kind: "None" };
```

This is a discriminated union — it works directly with `branch`:

```ts
pipe(
  lookup,
  branch({
    Some: pipe(extractField("value"), process),
    None: fallback,
  }),
)
```

## Proposed builtins

### `some` / `none` — constructors

```ts
function some<T>(): TypedAction<T, { kind: "Some"; value: T }>
// Equivalent to: tag("Some")

function none<T>(): TypedAction<T, { kind: "None" }>
// Drops the input, returns { kind: "None" }
```

`some` is just `tag("Some")`. `none` needs to drop the input and produce a fixed value — could be `pipe(drop(), constant({ kind: "None" }))` or a dedicated builtin.

### `filterMap` — map + flatten options

```ts
function filterMap<In, Out>(
  action: TypedAction<In, Option<Out>>,
): TypedAction<In[], Out[]>
```

For each element, runs the action. Collects only the `Some` values, discards `None`. This is the array equivalent of `flatMap` over options.

Desugars to: `forEach(action)` → `Option<Out>[]` → new `collectSome` builtin → `Out[]`.

### `unwrapOr` — provide a default for None

```ts
function unwrapOr<T>(defaultValue: T): TypedAction<Option<T>, T>
```

Desugars to:

```ts
branch({
  Some: extractField("value"),
  None: pipe(drop(), constant(defaultValue)),
})
```

Since this is just a branch, it might not need a dedicated combinator. But it's common enough that a named version improves readability.

## Where option types would be used

### In the refactor demos

`analyze` currently returns `Refactor[]` — an empty array means no refactors found. This works fine with `forEach` + `flatten`. Option types aren't needed here because the cardinality is already "zero or more."

Option types shine for single-value lookups:

```ts
// Instead of: lookup returns T | null (not representable in barnum)
// Use: lookup returns Option<T>
pipe(
  lookupUser,
  branch({
    Some: pipe(extractField("value"), processUser),
    None: pipe(drop(), constant({ error: "not found" })),
  }),
)
```

### In validation pipelines

```ts
pipe(
  validate,           // T → Option<ValidatedT>
  branch({
    Some: pipe(extractField("value"), save),
    None: pipe(drop(), logRejection),
  }),
)
```

## Implementation priority

Low. The discriminated union pattern (`{ kind: "Some" | "None" }`) already works with `branch`. The builtins (`some`, `none`, `filterMap`, `unwrapOr`) are convenience — they don't enable new capabilities, just reduce boilerplate.

The main value is standardization: if all handlers use `Option<T>` instead of ad-hoc unions, combinators like `filterMap` and `unwrapOr` compose naturally.

## Interaction with other features

- **`branch`**: Option types are just discriminated unions, so `branch` handles them directly.
- **`loop`**: `LoopResult<TContinue, TBreak>` is structurally similar (`Continue`/`Break` vs `Some`/`None`). Could share implementation.
- **`forEach`**: Arrays of options need `collectSome` or `filterMap` to extract values.
- **Thunk builtins**: `some` and `none` are zero-arg generics, so they benefit from the thunk pattern: `pipe(validate, branch({ Some: ..., None: none }))`.
