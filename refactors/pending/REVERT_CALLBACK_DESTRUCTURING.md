# Revert Callback Destructuring

## Context

Commits `a002b1b1` (withResource) and `1102ff74` (fold) changed these methods to accept callback parameters with VarRef destructuring. The new direction is: methods accept Pipeable, users opt into VarRef/destructuring via `bindInput` + `.split()`.

## What to Revert

### withResource (src/builtins/with-resource.ts)

Currently accepts:
```typescript
withResource<TIn, TResource, TOut>({
  create: Pipeable<TIn, TResource>,
  action: (resource: VarRef<TResource>, input: VarRef<TIn>) => BodyResult<TOut>,
  dispose: Pipeable<TResource, any>,
})
```

Should change `action` back to a Pipeable. Input type is `[TResource, TIn]` (a tuple).

User would write:
```typescript
withResource({
  create: createDb,
  action: bindInput<[Db, Config]>((state) => {
    const [db, config] = state.split();
    return db.then(query(config));
  }),
  dispose: closeDb,
})
```

### Iterator.fold (src/iterator.ts)

Currently accepts:
```typescript
fold<TElement, TAcc>(
  init: Pipeable<void, TAcc>,
  body: (acc: VarRef<TAcc>, element: VarRef<TElement>) => BodyResult<TAcc>,
)
```

Should change `body` back to `Pipeable<[TAcc, TElement], TAcc>`. User would write:
```typescript
Iterator.fold(
  constant(0),
  bindInput<[number, Item]>((state) => {
    const [acc, item] = state.split();
    return item.then(getScore).then(add(acc));
  }),
)
```

## `.split()` — Proxy-based destructuring

One method on TypedAction. Returns a Proxy that supports destructuring for both tuples and objects.

**VarRef itself is NOT iterable.** You cannot `for...of` or spread a VarRef. The Proxy returned by `.split()` uses `Symbol.iterator` internally because that's how JS destructuring syntax works, but this is an implementation detail of `.split()`, not a property of VarRef.

### Usage

```typescript
// Tuple destructuring
bindInput<[TAcc, TElement]>((state) => {
  const [acc, element] = state.split();
  return ...
})

// Object destructuring
bindInput<{ resource: Db, input: Config }>((state) => {
  const { resource, input } = state.split();
  return ...
})
```

### Type

```typescript
type Split<T> = { [K in keyof T]: VarRef<T[K]> };
```

Mapped type preserves tuple structure (TypeScript maps tuples positionally).

### Runtime

`.split()` returns a Proxy:
- `Symbol.iterator` → generator yielding `this.getIndex(i).unwrap()` (supports `[a, b] = x.split()` syntax)
- String property access → `this.getField(key)` (supports `{ a, b } = x.split()` syntax)

One Proxy, both destructuring patterns. Thin sugar over existing primitives (getIndex, getField, unwrap).

## Implementation Order

1. Add `.split()` to TypedAction (Proxy-based)
2. Revert `withResource` action param to Pipeable (input is `[TResource, TIn]`)
3. Revert `fold` body param to Pipeable (input is `[TAcc, TElement]`)
4. Update tests
