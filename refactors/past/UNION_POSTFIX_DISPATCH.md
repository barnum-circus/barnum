# Union Postfix Dispatch

## Problem

Postfix methods like `.map()`, `.flatten()`, `.unwrapOr()` exist for both Option and Result. Currently we either suffix them (`.mapOption()`, `.mapErr()`) or only support one type (`.unwrapOr()` is Result-only). This is inconsistent and ugly.

With dispatch, `.map()` just works regardless of whether the output is Option or Result.

## Two phases

**Phase 1 (this doc):** Add `__union` plumbing, constructors attach it, combinators propagate it, add new dispatched postfix methods. Existing `mapOption`, `mapErr`, `unwrapOr`, `flatten` stay as-is.

**Phase 2 (separate):** Rename `mapOption` → `map`, widen `unwrapOr`/`flatten` to dispatch, remove old names. This is a breaking change to the postfix API.

## Design

Every TypedAction gets an optional `__union` property — a reference to a methods object. Constructors (`Option.some`, `Result.ok`, etc.) attach it. New postfix methods check it and dispatch to the correct implementation.

## Overload validation

TypeScript `this`-constrained overloads resolve correctly for all dispatched methods. Validated with a scratch typecheck: `flatten` (Array/Option/Result), `map` (Option/Result), `andThen` (Option/Result), `unwrapOr` (Option/Result), `collect` (Option[]), `transpose` (both directions). Zero type errors, all overloads pick the correct signature based on self type.

---

## Phase 1: Before / After

### TypedAction type (`ast.ts`)

**Before:**
```ts
export type TypedAction<In = unknown, Out = unknown> = Action & {
  __in?: (input: In) => void;
  __in_co?: In;
  __out?: () => Out;
  then<TNext>(next: Pipeable<Out, TNext>): TypedAction<In, TNext>;
  forEach<TIn, TElement, TNext>(
    this: TypedAction<TIn, TElement[]>,
    action: Pipeable<TElement, TNext>,
  ): TypedAction<TIn, TNext[]>;
  branch<TCases extends ...>(cases: TCases): TypedAction<In, ...>;
  flatten(): TypedAction<In, Out extends (infer T)[][] ? T[] : Out>;
  drop(): TypedAction<In, void>;
  // ... structural methods ...
  mapOption<TIn, T, U>(
    this: TypedAction<TIn, Option<T>>,
    action: Pipeable<T, U>,
  ): TypedAction<TIn, Option<U>>;
  mapErr<TIn, TValue, TError, TErrorOut>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    action: Pipeable<TError, TErrorOut>,
  ): TypedAction<TIn, Result<TValue, TErrorOut>>;
  unwrapOr<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    defaultAction: Pipeable<TError, TValue>,
  ): TypedAction<TIn, TValue>;
};
```

**After:**
```ts
export type TypedAction<In = unknown, Out = unknown> = Action & {
  __in?: (input: In) => void;
  __in_co?: In;
  __out?: () => Out;
  __union?: UnionMethods;                              // NEW — runtime dispatch table
  then<TNext>(next: Pipeable<Out, TNext>): TypedAction<In, TNext>;
  forEach<TIn, TElement, TNext>(
    this: TypedAction<TIn, TElement[]>,
    action: Pipeable<TElement, TNext>,
  ): TypedAction<TIn, TNext[]>;
  branch<TCases extends ...>(cases: TCases): TypedAction<In, ...>;
  flatten(): TypedAction<In, Out extends (infer T)[][] ? T[] : Out>;
  drop(): TypedAction<In, void>;
  // ... structural methods ...

  // --- Existing (unchanged) ---
  mapOption<TIn, T, U>(
    this: TypedAction<TIn, Option<T>>,
    action: Pipeable<T, U>,
  ): TypedAction<TIn, Option<U>>;
  mapErr<TIn, TValue, TError, TErrorOut>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    action: Pipeable<TError, TErrorOut>,
  ): TypedAction<TIn, Result<TValue, TErrorOut>>;
  unwrapOr<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    defaultAction: Pipeable<TError, TValue>,
  ): TypedAction<TIn, TValue>;

  // --- NEW dispatched methods ---

  // Option
  andThen<TIn, TValue, TOut>(
    this: TypedAction<TIn, Option<TValue>>,
    action: Pipeable<TValue, Option<TOut>>,
  ): TypedAction<TIn, Option<TOut>>;
  // Result
  andThen<TIn, TValue, TOut, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    action: Pipeable<TValue, Result<TOut, TError>>,
  ): TypedAction<TIn, Result<TOut, TError>>;

  // Option-only
  filter<TIn, TValue>(
    this: TypedAction<TIn, Option<TValue>>,
    predicate: Pipeable<TValue, Option<TValue>>,
  ): TypedAction<TIn, Option<TValue>>;
  isSome<TIn, TValue>(this: TypedAction<TIn, Option<TValue>>): TypedAction<TIn, boolean>;
  isNone<TIn, TValue>(this: TypedAction<TIn, Option<TValue>>): TypedAction<TIn, boolean>;
  collect<TIn, TValue>(
    this: TypedAction<TIn, Option<TValue>[]>,
  ): TypedAction<TIn, TValue[]>;

  // Result-only
  or<TIn, TValue, TError, TErrorOut>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    fallback: Pipeable<TError, Result<TValue, TErrorOut>>,
  ): TypedAction<TIn, Result<TValue, TErrorOut>>;
  and<TIn, TValue, TOut, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
    other: Pipeable<void, Result<TOut, TError>>,
  ): TypedAction<TIn, Result<TOut, TError>>;
  toOption<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
  ): TypedAction<TIn, Option<TValue>>;
  toOptionErr<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
  ): TypedAction<TIn, Option<TError>>;
  isOk<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
  ): TypedAction<TIn, boolean>;
  isErr<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<TValue, TError>>,
  ): TypedAction<TIn, boolean>;

  // Transpose (dispatched — both directions)
  // Option<Result<T, E>> → Result<Option<T>, E>
  transpose<TIn, TValue, TError>(
    this: TypedAction<TIn, Option<Result<TValue, TError>>>,
  ): TypedAction<TIn, Result<Option<TValue>, TError>>;
  // Result<Option<T>, E> → Option<Result<T, E>>
  transpose<TIn, TValue, TError>(
    this: TypedAction<TIn, Result<Option<TValue>, TError>>,
  ): TypedAction<TIn, Option<Result<TValue, TError>>>;
};
```

Key changes:
- `mapOption`, `mapErr`, `unwrapOr`, `flatten` **stay as-is** (renamed in Phase 2)
- `__union` property added
- New postfix methods: `andThen`, `filter`, `isSome`, `isNone`, `collect`, `or`, `and`, `toOption`, `toOptionErr`, `transpose` (overloaded), `isOk`, `isErr`

---

### Methods objects (`builtins.ts`)

**Before:** doesn't exist.

**After:**
```ts
interface UnionMethods {
  map?: (action: Action) => Action;
  andThen?: (action: Action) => Action;
  unwrapOr?: (action: Action) => Action;
  flatten?: () => Action;
  // Option-only
  filter?: (predicate: Action) => Action;
  collect?: () => Action;
  isSome?: () => Action;
  isNone?: () => Action;
  // Result-only
  mapErr?: (action: Action) => Action;
  and?: (other: Action) => Action;
  or?: (fallback: Action) => Action;
  toOption?: () => Action;
  toOptionErr?: () => Action;
  transpose?: () => Action;
  isOk?: () => Action;
  isErr?: () => Action;
}

const optionMethods: UnionMethods = {
  map: (action) => Option.map(action),
  andThen: (action) => Option.andThen(action),
  unwrapOr: (action) => Option.unwrapOr(action),
  flatten: () => Option.flatten(),
  filter: (predicate) => Option.filter(predicate),
  collect: () => Option.collect(),
  isSome: () => Option.isSome(),
  isNone: () => Option.isNone(),
};

const resultMethods: UnionMethods = {
  map: (action) => Result.map(action),
  andThen: (action) => Result.andThen(action),
  unwrapOr: (action) => Result.unwrapOr(action),
  flatten: () => Result.flatten(),
  mapErr: (action) => Result.mapErr(action),
  and: (other) => Result.and(other),
  or: (fallback) => Result.or(fallback),
  toOption: () => Result.toOption(),
  toOptionErr: () => Result.toOptionErr(),
  transpose: () => Result.transpose(),
  isOk: () => Result.isOk(),
  isErr: () => Result.isErr(),
};
```

---

### `withUnion()` helper (`builtins.ts`)

**Before:** doesn't exist.

**After:**
```ts
function withUnion<In, Out>(action: TypedAction<In, Out>, methods: UnionMethods): TypedAction<In, Out> {
  action.__union = methods;
  return action;
}
```

Since `__union` is declared on `TypedAction`, no cast needed.

---

### Constructors attach `__union` (`builtins.ts`)

**Before:**
```ts
export const Option = {
  some<T>(): TypedAction<T, OptionT<T>> {
    return tag("Some") as TypedAction<T, OptionT<T>>;
  },
  none<T>(): TypedAction<any, OptionT<T>> {
    return tag("None") as TypedAction<any, OptionT<T>>;
  },
  // ...
};
```

**After:**
```ts
export const Option = {
  some<T>(): TypedAction<T, OptionT<T>> {
    return withUnion(tag("Some") as TypedAction<T, OptionT<T>>, optionMethods);
  },
  none<T>(): TypedAction<any, OptionT<T>> {
    return withUnion(tag("None") as TypedAction<any, OptionT<T>>, optionMethods);
  },
  // ...
};
```

Same pattern for `Result.ok`, `Result.err`.

---

### Combinators propagate `__union` (`builtins.ts`)

#### Classification: which combinators propagate?

**Propagate same family** (wrap output with `withUnion(result, sameMethods)`):
- `Option.map`, `Option.andThen`, `Option.flatten`, `Option.filter`
- `Result.map`, `Result.mapErr`, `Result.andThen`, `Result.flatten`, `Result.or`, `Result.and`

**Change family** (wrap output with `withUnion(result, newMethods)`):
- `Result.toOption` → `optionMethods`
- `Result.toOptionErr` → `optionMethods`
- `Result.transpose` → `optionMethods` (output is `Option<Result<...>>`)

**Exit union** (do NOT attach `__union`):
- `Option.unwrapOr`, `Option.isSome`, `Option.isNone`, `Option.collect`
- `Result.unwrapOr`, `Result.isOk`, `Result.isErr`

**Example — family-preserving:**
```ts
// Before
map<T, U>(action: Pipeable<T, U>): TypedAction<OptionT<T>, OptionT<U>> {
  return branch({
    Some: chain(action as any, tag("Some")),
    None: tag("None"),
  }) as TypedAction<OptionT<T>, OptionT<U>>;
},

// After
map<T, U>(action: Pipeable<T, U>): TypedAction<OptionT<T>, OptionT<U>> {
  return withUnion(
    branch({
      Some: chain(action as any, tag("Some")),
      None: tag("None"),
    }) as TypedAction<OptionT<T>, OptionT<U>>,
    optionMethods,
  );
},
```

**Example — family-changing:**
```ts
// Before
toOption<TValue, TError>(): TypedAction<ResultT<TValue, TError>, OptionT<TValue>> {
  return branch({
    Ok: tag("Some"),
    Err: drop.tag("None"),
  }) as TypedAction<ResultT<TValue, TError>, OptionT<TValue>>;
},

// After
toOption<TValue, TError>(): TypedAction<ResultT<TValue, TError>, OptionT<TValue>> {
  return withUnion(
    branch({
      Ok: tag("Some"),
      Err: drop.tag("None"),
    }) as TypedAction<ResultT<TValue, TError>, OptionT<TValue>>,
    optionMethods,  // output is Option, not Result
  );
},
```

---

### Standalone functions that produce Options (`builtins.ts`)

These are not in the Option namespace but produce `Option<T>` — they need `withUnion`:

```ts
// Before
export function splitFirst<TElement>(): TypedAction<TElement[], OptionT<[TElement, TElement[]]>> {
  return typedAction({ kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "SplitFirst" } } });
}

// After
export function splitFirst<TElement>(): TypedAction<TElement[], OptionT<[TElement, TElement[]]>> {
  return withUnion(
    typedAction({ kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "SplitFirst" } } }),
    optionMethods,
  );
}
```

Same for `splitLast`, `first`, `last`.

---

### New postfix method implementations (`ast.ts`)

All new postfix methods follow the same dispatch pattern. Only one cast per method: `as TypedAction` on the return (the overloaded type signature on `TypedAction` handles the real return type; the implementation just builds AST). Since `__union` is on the `TypedAction` type, `this.__union` works directly.

```ts
function andThenMethod(this: TypedAction, action: Action): TypedAction {
  const methods = this.__union;
  if (!methods?.andThen) throw new Error(".andThen() requires Option or Result output");
  return chain(this, methods.andThen(action)) as TypedAction;
}

function filterMethod(this: TypedAction, predicate: Action): TypedAction {
  const methods = this.__union;
  if (!methods?.filter) throw new Error(".filter() requires Option output");
  return chain(this, methods.filter(predicate)) as TypedAction;
}

function isSomeMethod(this: TypedAction): TypedAction {
  const methods = this.__union;
  if (!methods?.isSome) throw new Error(".isSome() requires Option output");
  return chain(this, methods.isSome()) as TypedAction;
}

function isNoneMethod(this: TypedAction): TypedAction {
  const methods = this.__union;
  if (!methods?.isNone) throw new Error(".isNone() requires Option output");
  return chain(this, methods.isNone()) as TypedAction;
}

function collectMethod(this: TypedAction): TypedAction {
  // collect is always Option.collect(). No dispatch needed.
  return chain(this, Option.collect()) as TypedAction;
}

function orMethod(this: TypedAction, fallback: Action): TypedAction {
  const methods = this.__union;
  if (!methods?.or) throw new Error(".or() requires Result output");
  return chain(this, methods.or(fallback)) as TypedAction;
}

function andMethod(this: TypedAction, other: Action): TypedAction {
  const methods = this.__union;
  if (!methods?.and) throw new Error(".and() requires Result output");
  return chain(this, methods.and(other)) as TypedAction;
}

function toOptionMethod(this: TypedAction): TypedAction {
  const methods = this.__union;
  if (!methods?.toOption) throw new Error(".toOption() requires Result output");
  return chain(this, methods.toOption()) as TypedAction;
}

function toOptionErrMethod(this: TypedAction): TypedAction {
  const methods = this.__union;
  if (!methods?.toOptionErr) throw new Error(".toOptionErr() requires Result output");
  return chain(this, methods.toOptionErr()) as TypedAction;
}

function transposeMethod(this: TypedAction): TypedAction {
  const methods = this.__union;
  if (!methods?.transpose) throw new Error(".transpose() requires Option<Result> or Result<Option> output");
  return chain(this, methods.transpose()) as TypedAction;
}

function isOkMethod(this: TypedAction): TypedAction {
  const methods = this.__union;
  if (!methods?.isOk) throw new Error(".isOk() requires Result output");
  return chain(this, methods.isOk()) as TypedAction;
}

function isErrMethod(this: TypedAction): TypedAction {
  const methods = this.__union;
  if (!methods?.isErr) throw new Error(".isErr() requires Result output");
  return chain(this, methods.isErr()) as TypedAction;
}
```

Registered in `typedAction()`:
```ts
Object.defineProperties(action, {
  // ... existing methods ...
  andThen: { value: andThenMethod, configurable: true },
  filter: { value: filterMethod, configurable: true },
  isSome: { value: isSomeMethod, configurable: true },
  isNone: { value: isNoneMethod, configurable: true },
  collect: { value: collectMethod, configurable: true },
  or: { value: orMethod, configurable: true },
  and: { value: andMethod, configurable: true },
  toOption: { value: toOptionMethod, configurable: true },
  toOptionErr: { value: toOptionErrMethod, configurable: true },
  transpose: { value: transposeMethod, configurable: true },
  isOk: { value: isOkMethod, configurable: true },
  isErr: { value: isErrMethod, configurable: true },
});
```

---

### `typedAction()` does NOT propagate `__union` (`ast.ts`)

Same as before — `typedAction()` only attaches postfix methods, never `__union`. The tag is set explicitly by `withUnion()` in constructors and combinators.

`.then()` also does NOT propagate `__union`. If you chain an arbitrary action after an Option-producing one, the output is no longer necessarily an Option:
```ts
Option.some().then(someHandler)  // __union is NOT copied to the result
```

---

## Phase 2: Rename (separate doc/commit)

After Phase 1 lands and is stable:

| Old postfix | New postfix | Notes |
|-------------|-------------|-------|
| `mapOption(action)` | `map(action)` | Overloaded: Option + Result |
| `mapErr(action)` | stays `mapErr(action)` | Result-only, no collision |
| `unwrapOr(action)` | stays `unwrapOr(action)` | Overloaded: Option + Result |
| `flatten()` | stays `flatten()` | Overloaded: Array + Option + Result |

The rename phase changes `mapOption` → `map` as an overloaded method and widens `unwrapOr` to accept Option (currently Result-only). `flatten` gets additional overloads for Option/Result. Existing `mapErr` stays as-is (Result-only, no naming conflict).

---

## What changes in Phase 1 (summary)

| Component | Before | After |
|-----------|--------|-------|
| `TypedAction` type | `mapOption`, `mapErr`, `unwrapOr` (Result-only) | Same + `__union`, `andThen`, `filter`, `isSome`, `isNone`, `collect`, `or`, `and`, `toOption`, `toOptionErr`, `transpose`, `isOk`, `isErr` |
| `__union` property | doesn't exist | `UnionMethods` reference on TypedAction |
| Option constructors | return plain TypedAction | attach `__union: optionMethods` |
| Result constructors | return plain TypedAction | attach `__union: resultMethods` |
| Option/Result combinators | return plain TypedAction | attach `__union` per classification above |
| `splitFirst`, `splitLast`, `first`, `last` | return plain TypedAction | attach `__union: optionMethods` |
| `.then()` | propagates nothing | still propagates nothing (correct) |
| `mapOption`, `mapErr`, `unwrapOr`, `flatten` | as-is | unchanged (Phase 2) |

## Files touched

1. **`ast.ts`** — Add `__union` to TypedAction type. Add new dispatched postfix method types and implementations. Register them in `typedAction()`.
2. **`builtins.ts`** — Add `UnionMethods` interface, `optionMethods`, `resultMethods` objects, `withUnion()` helper. Wrap all Option/Result constructors and family-preserving combinators with `withUnion()`. Wrap `splitFirst`, `splitLast`, `first`, `last` with `withUnion()`.
3. **`index.ts`** — No changes (no new exports needed; `UnionMethods` is internal).
