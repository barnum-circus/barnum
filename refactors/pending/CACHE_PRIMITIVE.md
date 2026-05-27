# Cache Primitive

## Question

Does `.split()` need a caching layer to prevent re-evaluation when used outside `bindInput`?

## Analysis

`bindInput` IS the caching primitive. Its AST nodes:
- `ResumeHandle` = "run this action once, store the result"
- `ResumePerform` = "recall the stored result"

When `.split()` is called on a VarRef (inside `bindInput`), each component (`ref.getIndex(0).unwrap()`, etc.) is a cheap field lookup off an already-captured value. No re-execution.

When `.split()` is called on a bare TypedAction (outside `bindInput`), each component re-evaluates the entire chain from root. For pure/cheap actions (`constant`, field access) this is fine. For effectful/expensive actions, this is wasteful.

## Why a new `.cache()` primitive doesn't help

A `.cache()` postfix would be semantically `action.bindInput(ref => ref)` — evaluate once, bind, pass through. But the caller still needs to fan out:

```typescript
// Hypothetical:
const [a, b] = expensiveAction.cache().split();
// cache() returns a TypedAction, not a VarRef.
// The proxy still chains getIndex off that TypedAction.
// Each use of a/b still re-evaluates cache() independently.
```

The Proxy returns split results synchronously — there's no scope to "bind into." You'd need:

```typescript
expensiveAction.bindInput((ref) => {
  const [a, b] = ref.split();
  // ref is captured once. a and b are cheap derivations.
})
```

Which is just the existing pattern.

## Conclusion

No new primitive needed. The convention:

- `.split()` is used on VarRefs inside `bindInput`
- `bindInput` is the user-facing opt-in for caching/fan-out
- If you need to split something expensive, wrap it in `bindInput` first

## Status

Resolved — no action needed. Documenting for reference.
