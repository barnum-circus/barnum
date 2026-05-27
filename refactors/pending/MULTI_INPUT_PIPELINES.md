# Multi-Input Pipelines

## Idea

Pipelines currently have one input type and one output type: `TypedAction<In, Out>`. What if they could have multiple inputs?

```typescript
// Single input (today):
bindInput<Word>((word) => word.then(capitalize))

// Multiple inputs (proposed):
bindInput<[User, Config]>((user, config) => 
  user.then(greet)
)
```

When `TIn` is a tuple, `bindInput` destructures the pipeline input into individual VarRefs — one per tuple element, passed as separate callback parameters.

## What This Means

Pipeline arity becomes a spectrum:
- Source = zero inputs: `() → T`
- Transform = one input: `A → B`
- Multi-input = N inputs: `(A, B, ...) → Out`

The Source/Transform distinction isn't a binary — it's arity.

## Relationship to Current System

- `.then()` connects one output to one input port
- `bind([a, b, c], ([x, y, z]) => ...)` already provides multiple VarRefs — but from multiple ACTIONS, not from one tuple input
- `bindInput<[A, B]>((a, b) => ...)` would destructure one tuple input into multiple VarRefs

## Disambiguation

If `TIn = [A, B]`, how to distinguish "destructure into two refs" vs "one ref to the whole tuple"?

Option: `[[A, B]]` wrapping means "single ref to `[A, B]`." `[A, B]` means "destructure." This mirrors Rust's tuple pattern matching.

## Status

Orthogonal to the current VarRef/callback work. Can be added retroactively without breaking existing APIs. The key insight is that this is a new way to think about pipeline inputs — not just a convenience feature.
