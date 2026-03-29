# Option types: representing optional values in barnum pipelines

> **Convention**: All discriminated unions use `{ kind: K; value: T }` form per TAGGED_UNION_CONVENTION.md. With PHANTOM_UNION_DEF.md, they use `TaggedUnion<Def>` for phantom `__def`. Branch auto-unwraps `value` — case handlers receive the payload directly.

## Problem

Many real workflows need to express "this step might not produce a value." Examples:

- `analyze` might find no refactoring opportunities for a file
- `typeCheck` might find no errors
- A lookup handler might not find a matching record
- A validation step might accept or reject input

Currently, handlers return arrays (empty = no results) or discriminated unions. There's no standardized option type or combinators that operate on it.

## Proposed option type

```ts
type OptionDef<T> = {
  Some: T;
  None: void;
};

type Option<T> = TaggedUnion<OptionDef<T>>;
// = { kind: "Some"; value: T; __def?: OptionDef<T> }
// | { kind: "None"; value: void; __def?: OptionDef<T> }
```

This is a tagged union — it works directly with `branch`. Branch auto-unwraps `value`, so case handlers receive the payload directly:

```ts
pipe(
  lookup,
  branch({
    Some: process,   // receives T directly (auto-unwrapped)
    None: fallback,  // receives void
  }),
)
```

## Proposed builtins

### `some` / `none` — constructors

```ts
function some<T>(): TypedAction<T, Option<T>>
// Equivalent to: tag<OptionDef<T>, "Some">("Some")

function none<T>(): TypedAction<unknown, Option<T>>
// Produces { kind: "None"; value: undefined } regardless of input
```

`some` is `tag<OptionDef<T>, "Some">("Some")` — tag knows the full union. `none` produces a fixed `{ kind: "None"; value: undefined }`.

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
function unwrapOr<T>(defaultAction: TypedAction<void, T>): TypedAction<Option<T>, T>
```

Takes an **action** (AST), not a raw value. Use `unwrapOr(constant("anonymous"))`, not `unwrapOr("anonymous")`.

Desugars to (with branch auto-unwrap):

```ts
branch({
  Some: identity(),  // receives T directly
  None: defaultAction,  // receives void, produces T
})
```

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
    Some: processUser,  // receives User directly (auto-unwrapped)
    None: pipe(drop(), constant({ error: "not found" })),
  }),
)
```

### In validation pipelines

```ts
pipe(
  validate,           // T → Option<ValidatedT>
  branch({
    Some: save,        // receives ValidatedT directly
    None: pipe(drop(), logRejection),
  }),
)
```

## Implementation priority

Low. The discriminated union pattern already works with `branch`. The builtins (`some`, `none`, `filterMap`, `unwrapOr`) are convenience — they don't enable new capabilities, just reduce boilerplate.

The main value is standardization: if all handlers use `Option<T>` instead of ad-hoc unions, combinators like `filterMap` and `unwrapOr` compose naturally.

## Interaction with other features

- **`branch`**: Option types are tagged unions, so `branch` handles them directly. Auto-unwraps `value`.
- **`loop`**: `LoopResult<TContinue, TBreak>` is structurally similar (`Continue`/`Break` vs `Some`/`None`). Both use `TaggedUnion`.
- **`forEach`**: Arrays of options need `collectSome` or `filterMap` to extract values.
- **Thunk builtins**: `some` and `none` are zero-arg generics, so they benefit from the thunk pattern: `pipe(validate, branch({ Some: ..., None: none }))`.
