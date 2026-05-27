# Revert Callback Destructuring

## Context

Commits `a002b1b1` (withResource) and `1102ff74` (fold) changed these methods to accept callback parameters with VarRef destructuring. The new direction is: methods accept Pipeable, users opt into VarRef/destructuring via `splitInput`/`bindInput` themselves.

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

Should change `action` back to a Pipeable. The question: what's the input type of `action`?

It receives both the resource AND the original input. So the input is `[TResource, TIn]` (a tuple), or an object `{ resource: TResource, input: TIn }`.

User would write:
```typescript
withResource({
  create: createDb,
  action: splitInput<[Db, Config]>((db, config) => db.then(query(config))),
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
  splitInput<[number, Item]>((acc, item) => 
    item.then(getScore).then(add(acc))
  ),
)
```

## New Primitive Needed

### splitInput (tuple destructuring)

```typescript
splitInput<[A, B]>((a: VarRef<A>, b: VarRef<B>) => BodyResult<Out>): TypedAction<[A, B], Out>
splitInput<[A, B, C]>((a, b, c) => ...): TypedAction<[A, B, C], Out>
```

Sugar for `bindInput` + `getIndex(0)`, `getIndex(1)`, etc.

## Implementation Order

1. Implement `splitInput`
2. Revert `withResource` action param to Pipeable (input is `[TResource, TIn]`)
3. Revert `fold` body param to Pipeable (input is `[TAcc, TElement]`)
4. Update tests
