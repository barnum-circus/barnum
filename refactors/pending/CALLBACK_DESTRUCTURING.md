# Callback Destructuring: VarRef Parameters for Tuple Inputs

## Motivation

Anywhere users receive a tuple from the framework, they must manually decompose it with `getIndex(n).unwrap()`. The unwrap can never fail (the tuple is always the right length) so it's pure ceremony. Worse, `getIndex` needs explicit type parameters because TypeScript can't infer the tuple type.

Compare the ergonomics:

```ts
// Current: manual tuple decomposition
iterator.fold(constant(null), getIndex<[null, PrStatus], 1>(1).unwrap().then(process))

// Proposed: framework destructures for you
iterator.fold(constant(null), (acc, element) => element.then(process))
```

The `loop` combinator already does this — `(recur, done) => body` gives you VarRefs directly. This refactor generalizes the pattern to every combinator that passes structured data to a user callback.

## Affected APIs

### `fold`

```ts
// Current
fold<TElement, TAcc>(
  init: Pipeable<void, TAcc>,
  body: Pipeable<[TAcc, TElement], TAcc>,
): TypedAction<Iterator<TElement>, TAcc>

// Proposed
fold<TElement, TAcc>(
  init: Pipeable<void, TAcc>,
  body: (acc: VarRef<TAcc>, element: VarRef<TElement>) => BodyResult<TAcc>,
): TypedAction<Iterator<TElement>, TAcc>
```

### `withResource`

```ts
// Current
withResource<TIn, TResource, TOut>({
  create: Pipeable<TIn, TResource>,
  action: Pipeable<[TResource, TIn], TOut>,
  dispose: Pipeable<TResource, TDisposeOut>,
})

// Proposed
withResource<TIn, TResource, TOut>({
  create: Pipeable<TIn, TResource>,
  action: (resource: VarRef<TResource>, input: VarRef<TIn>) => BodyResult<TOut>,
  dispose: Pipeable<TResource, TDisposeOut>,
})
```

### Any future combinator that exposes structured data

The pattern is universal: whenever the framework passes a fixed-structure tuple to user code, expose it as named VarRef parameters in a callback instead.

## Implementation

Each callback-style API internally wraps the user's body in a `bind` that extracts the tuple positions:

```ts
// fold implementation sketch
fold(init, userBody) {
  const internalBody = bind(
    [getIndex(0).unwrap(), getIndex(1).unwrap()],
    ([acc, element]) => pipe(drop, userBody(acc, element))
  );
  // ... existing fold machinery using internalBody as the step action
}
```

This is identical to how `loop` already works — it binds RestartHandle signals as VarRefs and passes them to the user's callback. The `getIndex(...).unwrap()` moves from user code into framework internals where it's invisible.

## Design notes

- **VarRefs are lazy.** Accessing `acc` or `element` in the body fires a ResumePerform that reads from the captured state. If the user never references one of the params (e.g., ignoring `acc` in a fold that only processes elements), no unnecessary work happens.
- **Type inference is perfect.** The callback parameters are typed as `VarRef<TAcc>` and `VarRef<TElement>` — the user gets full IntelliSense, postfix methods, `.pick()`, `.getField()`, everything.
- **Backward compatibility:** We don't care. No one is using this. Break freely.

## What this eliminates from user code

- `getIndex<[SomeType, OtherType], 0>(0).unwrap()` — gone
- `getIndex<[SomeType, OtherType], 1>(1).unwrap()` — gone
- `bindInput<[A, B]>((pair) => pair.getIndex(0).unwrap()...` — gone
- Explicit tuple type parameters on getIndex — gone

Users never write `getIndex` or deal with `Option` unwrapping for framework-provided tuples. Those are implementation details that leak into user code today.

## Priority

High. This is a significant ergonomics win that touches the most common user-facing patterns (fold, withResource). The implementation is mechanical — wrap existing body actions in bind + getIndex extraction.

-----

- agree, this seems doable now.
