# VarRef Callback Overloads

## Goal

Every method that currently accepts `Pipeable<TIn, TOut>` as its action/predicate parameter should ALSO accept `(element: VarRef<TIn>) => BodyResult<TOut>` — a callback that receives the element as a VarRef.

```typescript
// Both forms work:
items.iterate().map(foo)                          // existing Pipeable form
items.iterate().map(item => item.then(foo))       // new VarRef callback form
```

The callback form gives users access to the element as a VarRef, enabling them to reference it multiple times or combine it with other values.

## Type Design

A union type for the parameter:

```typescript
type ActionOrCallback<TIn, TOut> = Pipeable<TIn, TOut> | ((element: VarRef<TIn>) => BodyResult<TOut>);
```

At the type level, TypeScript will infer which branch the user is calling. At runtime, we detect callbacks with `typeof arg === "function"` and wrap them with `bindInput`.

## Runtime Normalization

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

## Methods to Change

### Iterator namespace (`src/iterator.ts`)

| Method | Current param | New param |
|--------|--------------|-----------|
| `map` | `action: Pipeable<TIn, TOut>` | `ActionOrCallback<TIn, TOut>` |
| `flatMap` | `action: Pipeable<TIn, unknown>` | callback returning IntoIterator |
| `filter` | `predicate: Pipeable<TElement, boolean>` | `ActionOrCallback<TElement, boolean>` |

### Option namespace (`src/option.ts`)

| Method | Current param | New param |
|--------|--------------|-----------|
| `map` | `action: Pipeable<T, U>` | `ActionOrCallback<T, U>` |
| `andThen` | `action: Pipeable<T, Option<U>>` | `ActionOrCallback<T, Option<U>>` |
| `filter` | `predicate: Pipeable<T, Option<T>>` | `ActionOrCallback<T, Option<T>>` |
| `unwrapOr` | `defaultAction: Pipeable<void, T>` | `ActionOrCallback<void, T>` |

### Result namespace (`src/result.ts`)

| Method | Current param | New param |
|--------|--------------|-----------|
| `map` | `action: Pipeable<TValue, TOut>` | `ActionOrCallback<TValue, TOut>` |
| `mapErr` | `action: Pipeable<TError, TErrorOut>` | `ActionOrCallback<TError, TErrorOut>` |
| `andThen` | `action: Pipeable<TValue, Result<TOut, TError>>` | callback form |
| `or` | `fallback: Pipeable<TError, Result<TValue, TErrorOut>>` | callback form |
| `unwrapOr` | `defaultAction: Pipeable<TError, TValue>` | callback form |

### TypedAction postfix methods (`src/ast.ts` type definition)

Every postfix `.map()`, `.andThen()`, `.filter()`, `.unwrapOr()`, `.mapErr()`, `.or()`, `.flatMap()` needs overloads for both forms.

### forEach (`src/ast.ts`)

```typescript
// Current:
export function forEach<In, Out>(action: Pipeable<In, Out>): TypedAction<Array<In>, Array<Out>>
// New — also accept callback:
export function forEach<In, Out>(action: ActionOrCallback<In, Out>): TypedAction<Array<In>, Array<Out>>
```

## Implementation Order

1. Define `ActionOrCallback` type and `normalizeToAction` helper (in bind.ts or a new shared location)
2. Update Iterator.map, flatMap, filter
3. Update Option.map, andThen, filter, unwrapOr
4. Update Result.map, mapErr, andThen, or, unwrapOr
5. Update forEach
6. Update TypedAction interface overloads in ast.ts
7. Update postfix method implementations in ast.ts
8. Add type tests demonstrating both forms
9. Verify all existing tests still pass

## Backward Compatibility

Fully backward compatible. The existing `Pipeable` form continues to work unchanged. The callback form is strictly additive.

## Open Questions

1. Should `normalizeToAction` live in `bind.ts` (since it uses `bindInput`) or somewhere else?
2. For `flatMap`, the callback return type is `IntoIterator` (Iterator | Option | Result | Array). The callback form would be `(element: VarRef<TIn>) => BodyResult<IntoIterator<TOut>>` — does this need special handling or does the existing `intoIteratorNormalize` compose naturally?
3. Should `race` branches also accept callbacks? (Probably not initially — race takes an array of pipelines, not a single action param.)
