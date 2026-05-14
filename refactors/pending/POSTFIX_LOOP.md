# Postfix `.loop()`

## Motivation

`loop` is currently standalone-only. Every other major combinator (`bindInput`, `earlyReturn`, `tryCatch`) has a postfix form. The standalone form forces wrapping in `pipe()` or awkward chaining:

```typescript
// Current: standalone, breaks the postfix chain
someAction.then(
  loop<TBreak, TRecur>((recur, done) => body)
)

// Desired: postfix, reads left-to-right
someAction.loop<TBreak>((recur, done) => body)
```

## Current state

`loop<TBreak, TRecur>` takes `TRecur` as the loop body's input type — this is what gets passed to the body on each iteration (and on the initial entry). The standalone form uses `PipeIn<TRecur>` as its overall input type, which collapses `void` → `any` so you can call `loop<string, void>(...)` without providing input.

## Proposed design

The postfix form chains the preceding action's output as the initial `TRecur`:

```typescript
// Type signature on TypedAction<In, Out>:
loop<TBreak>(
  this: TypedAction<In, Out>,
  bodyFn: (
    recur: TypedAction<Out, never>,
    done: TypedAction<VoidToNull<TBreak>, never>,
  ) => Pipeable<Out, never>,
): TypedAction<In, VoidToNull<TBreak>>;
```

The preceding action produces `Out`, which becomes `TRecur`. No need for a separate `TRecur` type parameter — it's inferred from the chain.

## Implementation

1. Add `loopMethod` function in `ast.ts` (same pattern as `bindInputMethod`)
2. Register in `defineProperties` 
3. Add type declaration on `TypedAction` interface

```typescript
function loopMethod(this: TypedAction, bodyFn: Function): TypedAction {
  // TRecur = this action's output, inferred by TypeScript
  return chain(toAction(this), toAction(loop(bodyFn as any)));
}
```

## Open questions

1. The `VoidToNull<TBreak>` in the return type doesn't simplify for generic `TBreak` — same issue that plagues the standalone form. Postfix doesn't fix this, but it doesn't make it worse either.
2. Should the postfix form still accept explicit `TBreak` type parameter, or can it be inferred from `done` usage? Likely needs to be explicit since TypeScript can't infer from callback argument usage.
