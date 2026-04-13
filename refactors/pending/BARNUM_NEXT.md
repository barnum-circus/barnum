# Barnum Next: Currying, allObject, Array Ops, Structural

## Motivation

The first cleanup pass (BARNUM_JS_CLEANUP.md, now in `past/`) handled naming, raw AST elimination, Rust builtin reduction, and first/last combinators. This document covers the remaining improvements: curried combinator APIs, `allObject`, array/iterable operations, and structural/architectural cleanup.

These are independent concerns. If any section grows complex enough to warrant its own approval cycle, split it out.

---

## 1. Curry `withTimeout`

**Current:** `withTimeout(ms, body)` — two positional args.

**File:** `libs/barnum/src/race.ts:139-163`

```ts
export function withTimeout<TIn, TOut>(
  ms: Pipeable<TIn, number>,
  body: Pipeable<TIn, TOut>,
): TypedAction<TIn, Result<TOut, void>>
```

**Proposed:** `withTimeout(ms)(body)` — curried, timeout first.

```ts
export function withTimeout<TIn>(
  ms: Pipeable<TIn, number>,
): <TOut>(body: Pipeable<TIn, TOut>) => TypedAction<TIn, Result<TOut, void>>
```

Partial application enables reusable timeout wrappers: `const withFiveSeconds = withTimeout(constant(5000))`. Composes naturally with `withRetries`:

```ts
withRetries(3)(withTimeout(constant(5000))(riskyStep))
```

The inner function closes over `ms` and constructs the same race AST as today. No new runtime behavior — just a calling convention change.

**Open question:** Should the `ms` parameter accept a bare `number` in addition to `Pipeable<TIn, number>`? A convenience overload `withTimeout(5000)` that wraps in `constant()` internally would reduce noise at call sites. The tradeoff is overload complexity.

---

## 2. `withRetries`

New combinator. Curried: `withRetries(count)(action)`.

```ts
function withRetries(
  count: number,
): <TIn, TOut>(action: Pipeable<TIn, TOut>) => TypedAction<TIn, TOut>
```

Retries the action up to `count` times on error. On success, returns the result. On final failure, re-throws.

**Implementation approach:** Loop with state `{ input: TIn, remaining: number }`. Each iteration: `tryCatch` the action. On success, `tag("Break")` with the result. On error, decrement remaining. If zero, re-throw. Otherwise `tag("Continue")` with decremented state.

```ts
function withRetries(count: number) {
  return <TIn, TOut>(action: Pipeable<TIn, TOut>): TypedAction<TIn, TOut> => {
    // Wrap input with retry counter
    // Loop: try action, break on success, decrement and continue on error
    // Final error: re-throw
  };
}
```

**v1:** No backoff. Just retry immediately.

**Future:** Accept config object `withRetries({ count: 3, delayMs: 1000 })` for fixed delay, `{ count: 3, backoff: "exponential", baseMs: 500 }` for exponential backoff. The curried form makes this backward-compatible — `withRetries(3)` still works alongside the config form.

**Dependency:** Needs `tryCatch` and `loop` working correctly. Both exist today.

---

## 3. `allObject`

JS convenience: takes a record of actions, runs all concurrently, returns a record of their outputs.

```ts
function allObject<TIn, TRecord extends Record<string, Pipeable<TIn, any>>>(
  record: TRecord,
): TypedAction<TIn, { [K in keyof TRecord]: PipeableOutput<TRecord[K]> }>
```

Example:
```ts
allObject({
  user: getUser,
  settings: getSettings,
  permissions: getPermissions,
})
// TypedAction<TIn, { user: User; settings: Settings; permissions: Permissions }>
```

**Implementation:** Same composition pattern as `pick()` — map each key to `chain(record[key], wrapInField(key))`, construct `All` node directly (bypasses overload limit), then `merge()`. Entirely JS-side, no new Rust builtins.

```ts
function allObject<TIn, TRecord extends Record<string, Pipeable<TIn, any>>>(
  record: TRecord,
): TypedAction<TIn, { [K in keyof TRecord]: PipeableOutput<TRecord[K]> }> {
  const keys = Object.keys(record);
  const wrappedActions = keys.map((key) =>
    chain(record[key] as any, wrapInField(key)) as Action,
  );
  const allAction: Action = { kind: "All", actions: wrappedActions };
  return chain(allAction as any, merge()) as any;
}
```

**Type extraction:** Needs a `PipeableOutput<P>` utility type that extracts `TOut` from `Pipeable<TIn, TOut>`. This is `P extends Pipeable<any, infer TOut> ? TOut : never`, or extracted from the phantom `__out` field.

---

## 4. Array/iterable operations

### Existing inventory

| Operation | Where | Signature |
|-----------|-------|-----------|
| `flatten` (array) | `builtins.ts` | `T[][] -> T[]` |
| `Option.flatten` | `builtins.ts` | `Option<Option<T>> -> Option<T>` |
| `Result.flatten` | `builtins.ts` | `Result<Result<V,E>,E> -> Result<V,E>` |
| `Option.collect` | `builtins.ts` (CollectSome builtin) | `Option<T>[] -> T[]` |
| `forEach` | `ast.ts` | `(T -> U) applied to T[] -> U[]` |
| `splitFirst` / `splitLast` | `builtins.ts` | head/tail, init/last decomposition |
| `first` / `last` | `builtins.ts` | `T[] -> Option<T>` |

### Missing operations

| Operation | Signature | Composition |
|-----------|-----------|-------------|
| `reverse` | `T[] -> T[]` | New Rust builtin (can't compose) |
| `concat` | `[T[], T[]] -> T[]` | `all(identity(), identity()).flatten()` or new builtin |
| `filter` | `(T -> Option<T>) applied to T[] -> T[]` | `forEach(predicate).then(Option.collect())` |
| `flatMap` | `(T -> U[]) applied to T[] -> U[]` | `forEach(action).then(flatten())` |

### `filter` pattern

Filter requires a predicate `T -> Option<T>` (Some to keep, None to discard):

```ts
function filter<T>(predicate: Pipeable<T, Option<T>>): TypedAction<T[], T[]> {
  return chain(forEach(predicate), Option.collect()) as any;
}
```

The `T -> Option<T>` signature is unusual but consistent — it's the filterMap pattern from Rust. A `T -> boolean` convenience wrapper would need `Bool.branch` (from PRIMITIVE_BUILTINS.md).

### `reverse` builtin

Can't be composed from existing primitives. Add to Rust BuiltinKind:

```rust
Reverse  // no fields
```

JS wrapper:
```ts
function reverse<T>(): TypedAction<T[], T[]> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Reverse" } },
  });
}
```

### Naming: `Option.collect` -> `Option.flatten`

`Option.collect()` takes `Option<T>[] -> T[]`. This is "flatten Option over Array." The name "collect" is confusing because Rust's `collect` is a more general operation. Rename to `Option.flatten()` — but this collides with the existing `Option.flatten()` for `Option<Option<T>> -> Option<T>`.

Options:
1. **`Option.filterSome()`** — descriptive, no collision
2. **`Option.collectSome()`** — matches the Rust builtin name
3. Keep `Option.collect()` — it's fine, the Rust analogy is loose anyway

Also rename the Rust `CollectSome` builtin to match whatever name is chosen.

---

## 5. Structural / architectural

### 5.1 Colocate tests

Tests are in `libs/barnum/tests/` instead of next to source files.

**Current:** `tests/patterns.test.ts` is a grab-bag of AST structure tests for pipe, all, branch, loop, bind, forEach, race, tryCatch — all in one file. Finding tests for a given combinator requires searching.

**Proposed:** Split into colocated files: `src/builtins.test.ts`, `src/pipe.test.ts`, `src/bind.test.ts`, `src/race.test.ts`, etc. Each test file tests exactly the module it sits next to.

The test helper `handlers.ts` stays in `tests/` or becomes `src/__test__/handlers.ts`.

Same principle on Rust side: tests for builtin execution should live next to the builtin implementation.

### 5.2 Reduce builtin definition boilerplate

Adding a new JS builtin requires touching five files: `ast.ts` (BuiltinKind type), `builtins.ts` (function), `index.ts` (re-export), plus Rust AST and Rust implementation.

**TS-side fix options:**

1. **`export *` from builtins.ts** — The barrel `index.ts` already re-exports from `builtins.ts`. The explicit list exists because `Option` and `Result` need declaration merging. Use `export * from "./builtins.js"` and only keep the declaration merge.

2. **Derive BuiltinKind from constructors** — The `BuiltinKind` type in `ast.ts` is maintained separately from the functions that construct those nodes. If builtins.ts is the single source of truth, the type union can be derived or simply not exported (it's an internal wire format).

### 5.3 List vs Array naming

Keep "array." The TS ecosystem universally uses Array/ReadonlyArray. Fighting the language's naming creates confusion. If a standalone syntax emerges, "list" could be the surface syntax name that compiles to array operations underneath.

---

## Dependency order

1. **Sections 1-3** (currying, allObject) are independent of each other.
2. **Section 4** (array ops) is independent but benefits from landing after section 3 (allObject establishes the pattern for record-based composition).
3. **Section 5** (structural) is independent of everything else.

No blocking dependencies between sections. Implement in any order.

---

## Overlap with other refactor docs

- **`PRIMITIVE_BUILTINS.md`** — Covers math, boolean, string, array, object builtins. Section 4 here overlaps with the array portion. This doc focuses on composition of existing primitives; PRIMITIVE_BUILTINS covers adding new categories.
- **`VOID_VS_NEVER.md`** — `drop`, `sleep`, and postfix `.drop()` return types are affected by the void-vs-never decision. Section 1 (`withTimeout`) uses `sleep` and `drop` internally.
- **`CONSOLIDATE_PHANTOM_FIELDS.md`** — Phantom field simplification affects all combinator signatures. Landing that first would simplify new combinators in sections 1-3.
- **`TS_VS_RUST_TRANSFORMS.md`** — Section 4's `reverse` builtin is a case study for that doc's decision framework.
