# Option types: representing optional values in barnum pipelines

> **Convention**: All discriminated unions use `TaggedUnion<Def>` — every variant carries `{ kind: K; value: T; __def?: Def }`. This is not optional: **all union variants must carry `__def`**, no exceptions. Constructors like `some()`, `none()`, `recur()`, `done()`, and `tag()` all require the full variant map as a type parameter so the output type carries `__def`. Branch uses `ExtractDef` for inference and auto-unwraps `value` before each case handler.

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

Both carry the full `OptionDef<T>` so the output includes `__def`:

```ts
function some<T>(): TypedAction<T, Option<T>>
// = tag<OptionDef<T>, "Some">("Some")
// Input: T, Output: Option<T> (full union with __def)

function none<T>(): TypedAction<void, Option<T>>
// = tag<OptionDef<T>, "None">("None")
// Input: void, Output: Option<T> (full union with __def)
```

Both use `tag()` with the full variant map. The input type comes from the def: `OptionDef<T>["Some"]` = `T`, `OptionDef<T>["None"]` = `void`. The output is `Option<T>` = `TaggedUnion<OptionDef<T>>` — carrying `__def` so `.branch()` can decompose it.

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
