# Eliminate `as TypedAction` Casts

## Motivation

`option.ts`, `result.ts`, and several other files cast `branch()` return values with `as TypedAction<...>`. These casts are unnecessary — `branch` already infers `BranchInput` and `ExtractOutput` from its handlers. The casts exist because inner combinators (`identity()`, `Option.none()`, `constant()`, etc.) are called without explicit type parameters, so TypeScript infers `unknown` or `any` and the output type collapses.

The fix is mechanical: pass explicit type parameters to leaf combinators so `branch` can infer the correct types. No runtime changes, no new abstractions — just tighter types.

## Root cause

`branch` infers its return type from the handlers:

```ts
function branch<TCases extends Record<string, Action>>(
  cases: TCases,
): TypedAction<BranchInput<TCases>, ExtractOutput<TCases[keyof TCases & string]>>
```

`ExtractOutput` unions the `__out` phantom of all handlers. When two handlers return `OptionT<U>` and `OptionT<unknown>` (from an unparameterized `Option.none()`), the union collapses to `OptionT<unknown>`. The cast forces it back.

With explicit type params on the leaf combinators, both handlers return `OptionT<U>`, `ExtractOutput` infers `OptionT<U>`, and the cast is unnecessary.

## Example

```ts
// BEFORE — Option.map:
map<T, U>(action: Pipeable<T, U>): TypedAction<OptionT<T>, OptionT<U>> {
  return branch({
    Some: chain(toAction(action), toAction(Option.some())),   // some<???> → OptionT<unknown>
    None: Option.none(),                                      // none<???> → OptionT<unknown>
  }) as TypedAction<OptionT<T>, OptionT<U>>;
}

// AFTER:
map<T, U>(action: Pipeable<T, U>): TypedAction<OptionT<T>, OptionT<U>> {
  return branch({
    Some: chain(toAction(action), toAction(Option.some<U>())),
    None: Option.none<U>(),
  });  // branch infers TypedAction<OptionT<T>, OptionT<U>> — no cast
}
```

---

## Affected files

### `libs/barnum/src/option.ts` (10 casts)

| Method | Leaf combinators needing type params |
|--------|--------------------------------------|
| `map` | `Option.some<U>()`, `Option.none<U>()` |
| `andThen` | `Option.none<U>()` |
| `unwrap` | `identity<T>()` |
| `unwrapOr` | `identity<T>()` |
| `filter` | `Option.none<T>()` |
| `isSome` | `constant<boolean>(true)`, `constant<boolean>(false)` |
| `isNone` | `constant<boolean>(false)`, `constant<boolean>(true)` |
| `transpose` | `Option.some<ResultT<...>>()`, `Result.ok<OptionT<...>>()`, `Result.err<...>()`, `Option.none<ResultT<...>>()` |
| `first` | Depends on `chain` inference — investigate |
| `last` | Same as `first` |

### `libs/barnum/src/result.ts` (12 casts)

| Method | Leaf combinators needing type params |
|--------|--------------------------------------|
| `map` | `Result.ok<TOut, TError>()`, `Result.err<TOut, TError>()` |
| `mapErr` | `Result.ok<TValue, TErrorOut>()`, `Result.err<TValue, TErrorOut>()` |
| `andThen` | `Result.err<TOut, TError>()` |
| `or` | `Result.ok<TValue, TErrorOut>()` |
| `unwrap` | `identity<TValue>()` |
| `unwrapOr` | `identity<TValue>()` |
| `asOkOption` | `Option.some<TValue>()`, `Option.none<TValue>()` |
| `asErrOption` | `Option.none<TError>()`, `Option.some<TError>()` |
| `transpose` | `Result.ok<...>()`, `Option.some<...>()`, `Option.none<...>()`, `Result.err<...>()` |
| `isOk` | `constant<boolean>(true)`, `constant<boolean>(false)` |
| `isErr` | `constant<boolean>(false)`, `constant<boolean>(true)` |

### `libs/barnum/src/ast.ts` (2 casts)

- Line 513: `chain(this, next) as TypedAction<TIn, TNext>` — depends on `chain`'s return type inference. If `chain` has proper generics this may resolve; investigate.
- Line 762: `action as TypedAction<In, Out>` — this is `typedAction()` wrapping a raw `Action`. Cast is structurally necessary (raw Action has no phantom types).

### `libs/barnum/src/builtins/tagged-union.ts` (1 cast)

- Line 36: `tag()` return — similar to `typedAction()`, may be structurally necessary since `tag` constructs from raw AST.

### `libs/barnum/src/builtins/struct.ts` (1 cast)

- Line 69: `augment` — depends on `chain` + `merge` inference. Investigate.

### `libs/barnum/src/builtins/with-resource.ts` (1 cast)

- Line 65: `withResource` — complex composition. Investigate.

---

## Implementation

### Task 1: option.ts — eliminate casts

Add explicit type params to all leaf combinators in `Option` namespace methods. Remove every `as TypedAction<...>` cast. Typecheck to verify `branch` infers correctly.

### Task 2: result.ts — eliminate casts

Same treatment for `Result` namespace methods.

### Task 3: ast.ts / builtins — investigate remaining casts

The casts in `ast.ts`, `tagged-union.ts`, `struct.ts`, and `with-resource.ts` may be structurally necessary (wrapping raw `Action` into `TypedAction`). Investigate whether `chain`'s generics can propagate types. Remove casts where possible; document why remaining casts are necessary.

---

## Verification

`pnpm run typecheck --output-logs=errors-only` from the repo root. If any cast removal causes a type error, the leaf combinator is missing a type param — add it. No runtime behavior changes.
