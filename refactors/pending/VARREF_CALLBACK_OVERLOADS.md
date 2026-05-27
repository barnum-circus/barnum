# VarRef Callback Overloads

## Goal

Every method that accepts a `Pipeable<TIn, TOut>` as its action/predicate parameter should ALSO accept `(element: VarRef<TIn>) => BodyResult<TOut>` — a callback that receives the element as a VarRef.

```typescript
// Both forms work:
items.iterate().map(foo); // existing Pipeable form
items.iterate().map((item) => item.then(foo)); // new VarRef callback form
```

The callback form gives users access to the element as a VarRef, enabling them to reference it multiple times or combine it with other values.

## Type Design

A union type for the parameter:

```typescript
type ActionOrCallback<TIn, TOut> =
  | Pipeable<TIn, TOut>
  | ((element: VarRef<TIn>) => BodyResult<TOut>);
```

At the type level, TypeScript will infer which branch the user is calling. At runtime, we detect callbacks with `typeof arg === "function"` and wrap them with `bindInput`.

## Methods to Change

### Postfix methods on TypedAction (`src/ast.ts`)

| Method                         | Current param                                           | New form                                                                                                                  |
| ------------------------------ | ------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| `.then<TNext>(next)`           | `next: Pipeable<Out, TNext>`                            | `next: Pipeable<Out, TNext> \| (element: VarRef<Out>) => BodyResult<TNext>`                                               |
| `.map(action)` (Option)        | `action: Pipeable<T, U>`                                | `action: Pipeable<T, U> \| (element: VarRef<T>) => BodyResult<U>`                                                         |
| `.map(action)` (Result)        | `action: Pipeable<TValue, TOut>`                        | `action: Pipeable<TValue, TOut> \| (element: VarRef<TValue>) => BodyResult<TOut>`                                         |
| `.map(action)` (Iterator)      | `action: Pipeable<TElement, TOut>`                      | `action: Pipeable<TElement, TOut> \| (element: VarRef<TElement>) => BodyResult<TOut>`                                     |
| `.mapErr(action)`              | `action: Pipeable<TError, TErrorOut>`                   | `action: Pipeable<TError, TErrorOut> \| (element: VarRef<TError>) => BodyResult<TErrorOut>`                               |
| `.unwrapOr(default)` (Option)  | `defaultAction: Pipeable<void, TValue>`                 | `defaultAction: Pipeable<void, TValue> \| (() => BodyResult<TValue>)`                                                     |
| `.unwrapOr(default)` (Result)  | `defaultAction: Pipeable<TError, TValue>`               | `defaultAction: Pipeable<TError, TValue> \| (err: VarRef<TError>) => BodyResult<TValue>`                                  |
| `.andThen(action)` (Option)    | `action: Pipeable<TValue, Option<TOut>>`                | `action: Pipeable<TValue, Option<TOut>> \| (element: VarRef<TValue>) => BodyResult<Option<TOut>>`                         |
| `.andThen(action)` (Result)    | `action: Pipeable<TValue, Result<TOut, TError>>`        | `action: Pipeable<TValue, Result<TOut, TError>> \| (element: VarRef<TValue>) => BodyResult<Result<TOut, TError>>`         |
| `.filter(pred)` (Option)       | `predicate: Pipeable<TValue, Option<TValue>>`           | `predicate: Pipeable<TValue, Option<TValue>> \| (element: VarRef<TValue>) => BodyResult<Option<TValue>>`                  |
| `.filter(pred)` (Iterator)     | `predicate: Pipeable<TElement, boolean>`                | `predicate: Pipeable<TElement, boolean> \| (element: VarRef<TElement>) => BodyResult<boolean>`                            |
| `.or(fallback)`                | `fallback: Pipeable<TError, Result<TValue, TErrorOut>>` | `fallback: Pipeable<TError, Result<TValue, TErrorOut>> \| (err: VarRef<TError>) => BodyResult<Result<TValue, TErrorOut>>` |
| `.flatMap(action)` (→Iterator) | `action: Pipeable<TElement, Iterator<TOut>>`            | `action: Pipeable<TElement, Iterator<TOut>> \| (element: VarRef<TElement>) => BodyResult<Iterator<TOut>>`                 |
| `.flatMap(action)` (→Option)   | `action: Pipeable<TElement, Option<TOut>>`              | `action: Pipeable<TElement, Option<TOut>> \| (element: VarRef<TElement>) => BodyResult<Option<TOut>>`                     |
| `.flatMap(action)` (→Result)   | `action: Pipeable<TElement, Result<TOut, TError>>`      | `action: Pipeable<TElement, Result<TOut, TError>> \| (element: VarRef<TElement>) => BodyResult<Result<TOut, TError>>`     |
| `.flatMap(action)` (→Array)    | `action: Pipeable<TElement, Array<TOut>>`               | `action: Pipeable<TElement, Array<TOut>> \| (element: VarRef<TElement>) => BodyResult<Array<TOut>>`                       |
| `.tap(action)`                 | `action: Pipeable<Out, any>`                            | `action: Pipeable<Out, any> \| (element: VarRef<Out>) => BodyResult<any>`                                                 |

### Iterator namespace (`src/iterator.ts`)

| Method                                 | Current signature                        | New param                             |
| -------------------------------------- | ---------------------------------------- | ------------------------------------- |
| `Iterator.map<TIn, TOut>(action)`      | `action: Pipeable<TIn, TOut>`            | `ActionOrCallback<TIn, TOut>`         |
| `Iterator.flatMap<TIn, TOut>(action)`  | `action: Pipeable<TIn, unknown>`         | `ActionOrCallback<TIn, unknown>`      |
| `Iterator.filter<TElement>(predicate)` | `predicate: Pipeable<TElement, boolean>` | `ActionOrCallback<TElement, boolean>` |

Note: `Iterator.fold` already uses VarRef callback form for its `body` param. Its `init: Pipeable<void, TAcc>` is a producer (no meaningful input), VarRef form N/A.

### Option namespace (`src/option.ts`)

| Method                              | Current signature                   | New param                                    |
| ----------------------------------- | ----------------------------------- | -------------------------------------------- |
| `Option.map<T, U>(action)`          | `action: Pipeable<T, U>`            | `ActionOrCallback<T, U>`                     |
| `Option.andThen<T, U>(action)`      | `action: Pipeable<T, Option<U>>`    | `ActionOrCallback<T, Option<U>>`             |
| `Option.filter<T>(predicate)`       | `predicate: Pipeable<T, Option<T>>` | `ActionOrCallback<T, Option<T>>`             |
| `Option.unwrapOr<T>(defaultAction)` | `defaultAction: Pipeable<void, T>`  | `Pipeable<void, T> \| (() => BodyResult<T>)` |

### Result namespace (`src/result.ts`)

| Method                                             | Current signature                                       | New param                                             |
| -------------------------------------------------- | ------------------------------------------------------- | ----------------------------------------------------- |
| `Result.map<TValue, TOut, TError>(action)`         | `action: Pipeable<TValue, TOut>`                        | `ActionOrCallback<TValue, TOut>`                      |
| `Result.mapErr<TValue, TError, TErrorOut>(action)` | `action: Pipeable<TError, TErrorOut>`                   | `ActionOrCallback<TError, TErrorOut>`                 |
| `Result.andThen<TValue, TOut, TError>(action)`     | `action: Pipeable<TValue, Result<TOut, TError>>`        | `ActionOrCallback<TValue, Result<TOut, TError>>`      |
| `Result.or<TValue, TError, TErrorOut>(fallback)`   | `fallback: Pipeable<TError, Result<TValue, TErrorOut>>` | `ActionOrCallback<TError, Result<TValue, TErrorOut>>` |
| `Result.unwrapOr<TValue, TError>(defaultAction)`   | `defaultAction: Pipeable<TError, TValue>`               | `ActionOrCallback<TError, TValue>`                    |

### Standalone functions

| Function                   | File     | Current signature           | New param                   |
| -------------------------- | -------- | --------------------------- | --------------------------- |
| `forEach<In, Out>(action)` | `ast.ts` | `action: Pipeable<In, Out>` | `ActionOrCallback<In, Out>` |
| `tap<T>(action)`           | `ast.ts` | `action: Pipeable<T, any>`  | `ActionOrCallback<T, any>`  |

## Runtime Normalization

At runtime, detect callbacks and wrap:

```typescript
function normalizeToAction<TIn, TOut>(
  actionOrCallback: ActionOrCallback<TIn, TOut>,
): Pipeable<TIn, TOut> {
  if (typeof actionOrCallback === "function") {
    return bindInput<TIn, TOut>(actionOrCallback);
  }
  return actionOrCallback;
}
```

This lives alongside `bindInput` (in `ast.ts` or `bind.ts`).

## Implementation Order

1. Define `ActionOrCallback<TIn, TOut>` type alias
2. Add `normalizeToAction` helper
3. Update namespace methods (Iterator.map/flatMap/filter, Option.map/andThen/filter/unwrapOr, Result.map/mapErr/andThen/or/unwrapOr)
4. Update standalone functions (forEach, tap)
5. Update TypedAction interface overloads in ast.ts
6. Update postfix method implementations in ast.ts
7. Add type tests demonstrating both forms
8. Verify all existing tests still pass

## Backward Compatibility

Don't care. No one is using this. Break freely. No dead code.

## Open Questions

1. Should `.then()` support the callback form? It's the lowest-level connector and every other method desugars through it. Adding VarRef support is trivial but may be noisy for no real benefit since `.then(x)` is already concise.
2. For `Option.unwrapOr` the input is `void` — the callback form becomes `() => BodyResult<T>` (no VarRef arg since there's nothing to reference). Is this useful enough to bother, or should we skip it?
3. For `.flatMap()`, the callback return type varies (Iterator | Option | Result | Array). The existing `intoIteratorNormalize` at runtime handles all cases — does this compose naturally with `bindInput`, or does the normalization need to happen after `bindInput`?
