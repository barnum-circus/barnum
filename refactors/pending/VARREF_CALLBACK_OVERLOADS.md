# VarRef Callback Params

## Goal

Every method that currently accepts a `Pipeable<TIn, TOut>` as its action/predicate parameter should change to accept `(element: VarRef<TIn>) => BodyResult<TOut>` instead. One way to do things. No union, no overloads.

```typescript
// Before:
items.iterate().map(foo);
// After:
items.iterate().map((item) => item.then(foo));
```

## Runtime

At runtime, the callback is wrapped with `bindInput`:

```typescript
// In the implementation of each method:
const action = bindInput<TIn, TOut>(callback);
```

## Methods to Change

### Postfix methods on TypedAction (`src/ast.ts`)

| Method                          | Current param                                           | New param                                                                  |
| ------------------------------- | ------------------------------------------------------- | -------------------------------------------------------------------------- |
| `.then<TNext>(next)`            | `next: Pipeable<Out, TNext>`                            | `next: (element: VarRef<Out>) => BodyResult<TNext>`                        |
| `.map(action)` (Option)         | `action: Pipeable<T, U>`                                | `action: (element: VarRef<T>) => BodyResult<U>`                            |
| `.map(action)` (Result)         | `action: Pipeable<TValue, TOut>`                        | `action: (element: VarRef<TValue>) => BodyResult<TOut>`                    |
| `.map(action)` (Iterator)       | `action: Pipeable<TElement, TOut>`                      | `action: (element: VarRef<TElement>) => BodyResult<TOut>`                  |
| `.mapErr(action)`               | `action: Pipeable<TError, TErrorOut>`                   | `action: (err: VarRef<TError>) => BodyResult<TErrorOut>`                   |
| `.unwrapOr(default)` (Option)   | `defaultAction: Pipeable<void, TValue>`                 | `defaultAction: () => BodyResult<TValue>`                                  |
| `.unwrapOr(default)` (Result)   | `defaultAction: Pipeable<TError, TValue>`               | `defaultAction: (err: VarRef<TError>) => BodyResult<TValue>`               |
| `.andThen(action)` (Option)     | `action: Pipeable<TValue, Option<TOut>>`                | `action: (element: VarRef<TValue>) => BodyResult<Option<TOut>>`            |
| `.andThen(action)` (Result)     | `action: Pipeable<TValue, Result<TOut, TError>>`        | `action: (element: VarRef<TValue>) => BodyResult<Result<TOut, TError>>`    |
| `.filter(pred)` (Option)        | `predicate: Pipeable<TValue, Option<TValue>>`           | `predicate: (element: VarRef<TValue>) => BodyResult<Option<TValue>>`       |
| `.filter(pred)` (Iterator)      | `predicate: Pipeable<TElement, boolean>`                | `predicate: (element: VarRef<TElement>) => BodyResult<boolean>`            |
| `.or(fallback)`                 | `fallback: Pipeable<TError, Result<TValue, TErrorOut>>` | `fallback: (err: VarRef<TError>) => BodyResult<Result<TValue, TErrorOut>>` |
| `.flatMap(action)` (->Iterator) | `action: Pipeable<TElement, Iterator<TOut>>`            | `action: (element: VarRef<TElement>) => BodyResult<Iterator<TOut>>`        |
| `.flatMap(action)` (->Option)   | `action: Pipeable<TElement, Option<TOut>>`              | `action: (element: VarRef<TElement>) => BodyResult<Option<TOut>>`          |
| `.flatMap(action)` (->Result)   | `action: Pipeable<TElement, Result<TOut, TError>>`      | `action: (element: VarRef<TElement>) => BodyResult<Result<TOut, TError>>`  |
| `.flatMap(action)` (->Array)    | `action: Pipeable<TElement, Array<TOut>>`               | `action: (element: VarRef<TElement>) => BodyResult<Array<TOut>>`           |
| `.tap(action)`                  | `action: Pipeable<Out, any>`                            | `action: (element: VarRef<Out>) => BodyResult<any>`                        |

### Iterator namespace (`src/iterator.ts`)

| Method                                 | Current param                            | New param                                                       |
| -------------------------------------- | ---------------------------------------- | --------------------------------------------------------------- |
| `Iterator.map<TIn, TOut>(action)`      | `action: Pipeable<TIn, TOut>`            | `action: (element: VarRef<TIn>) => BodyResult<TOut>`            |
| `Iterator.flatMap<TIn, TOut>(action)`  | `action: Pipeable<TIn, unknown>`         | `action: (element: VarRef<TIn>) => BodyResult<unknown>`         |
| `Iterator.filter<TElement>(predicate)` | `predicate: Pipeable<TElement, boolean>` | `predicate: (element: VarRef<TElement>) => BodyResult<boolean>` |

Note: `Iterator.fold` already uses VarRef callback form. No change needed.

### Option namespace (`src/option.ts`)

| Method                              | Current param                       | New param                                                  |
| ----------------------------------- | ----------------------------------- | ---------------------------------------------------------- |
| `Option.map<T, U>(action)`          | `action: Pipeable<T, U>`            | `action: (element: VarRef<T>) => BodyResult<U>`            |
| `Option.andThen<T, U>(action)`      | `action: Pipeable<T, Option<U>>`    | `action: (element: VarRef<T>) => BodyResult<Option<U>>`    |
| `Option.filter<T>(predicate)`       | `predicate: Pipeable<T, Option<T>>` | `predicate: (element: VarRef<T>) => BodyResult<Option<T>>` |
| `Option.unwrapOr<T>(defaultAction)` | `defaultAction: Pipeable<void, T>`  | `defaultAction: () => BodyResult<T>`                       |

### Result namespace (`src/result.ts`)

| Method                                             | Current param                                           | New param                                                                  |
| -------------------------------------------------- | ------------------------------------------------------- | -------------------------------------------------------------------------- |
| `Result.map<TValue, TOut, TError>(action)`         | `action: Pipeable<TValue, TOut>`                        | `action: (value: VarRef<TValue>) => BodyResult<TOut>`                      |
| `Result.mapErr<TValue, TError, TErrorOut>(action)` | `action: Pipeable<TError, TErrorOut>`                   | `action: (err: VarRef<TError>) => BodyResult<TErrorOut>`                   |
| `Result.andThen<TValue, TOut, TError>(action)`     | `action: Pipeable<TValue, Result<TOut, TError>>`        | `action: (value: VarRef<TValue>) => BodyResult<Result<TOut, TError>>`      |
| `Result.or<TValue, TError, TErrorOut>(fallback)`   | `fallback: Pipeable<TError, Result<TValue, TErrorOut>>` | `fallback: (err: VarRef<TError>) => BodyResult<Result<TValue, TErrorOut>>` |
| `Result.unwrapOr<TValue, TError>(defaultAction)`   | `defaultAction: Pipeable<TError, TValue>`               | `defaultAction: (err: VarRef<TError>) => BodyResult<TValue>`               |

### Standalone functions

| Function                   | Current param               | New param                                          |
| -------------------------- | --------------------------- | -------------------------------------------------- |
| `forEach<In, Out>(action)` | `action: Pipeable<In, Out>` | `action: (element: VarRef<In>) => BodyResult<Out>` |
| `tap<T>(action)`           | `action: Pipeable<T, any>`  | `action: (element: VarRef<T>) => BodyResult<any>`  |

## Implementation Order

1. Update namespace methods (Iterator, Option, Result) — wrap callbacks with `bindInput`
2. Update standalone functions (forEach, tap)
3. Update TypedAction interface (postfix method signatures)
4. Update postfix method implementations
5. Update all tests and demos to use callback form
6. Verify all tests pass

## Decision: Deferred

After discussion, the conclusion is:
- Methods (`map`, `filter`, etc.) stay as-is — accept Pipeable directly
- `bindInput` is user-facing: the explicit opt-in when you need VarRefs/fan-out
- Users pass `bindInput(...)` as the Pipeable argument when they need the callback form
- No method signature changes needed

This refactor is deferred. If we later want methods to accept callbacks directly, it's a backwards-compatible addition (overload). See also:
- `VARREF_AS_PIPELINE_PRIMITIVE.md` — exploration of Source/Transform distinction
- `MULTI_INPUT_PIPELINES.md` — multi-input / tuple destructuring idea
