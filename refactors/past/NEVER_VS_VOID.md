# Why `never` is correct (not `void`)

## The question

`get()` (in withState), `bind` VarRefs, and other "doesn't consume input" actions are typed as `TypedAction<never, T>`. Should `void` be used instead? Would it fix type inference issues?

## Analysis

With invariant types, `TypedAction<A, X>` is assignable to `TypedAction<B, X>` only when `A = B`:

| Input type | `__in` (covariant) | `__phantom_in` (contravariant) | Assignable to `TypedAction<string, X>`? |
|---|---|---|---|
| `never` | `never extends string` ✓ | `string extends never` ✗ | No (contravariant fails) |
| `void` | `void extends string` ✗ | `string extends void` ✗ | No (both fail) |
| `unknown` | `unknown extends string` ✗ | `string extends unknown` ✓ | No (covariant fails) |

`never` is the best choice — it's the only one where at least the covariant direction works. `void` is strictly worse (fails both sides). The pipe inference issues come from invariance + TypeScript resolving each argument's generics independently, not from the choice of `never`.

Semantically, `never` is also correct: `get()` genuinely doesn't need input. The engine passes a pipeline value at runtime, but the handler ignores it. `never` says "no specific input type is required" — the function can appear anywhere in a pipeline regardless of what precedes it. `void` would incorrectly say "the input must be undefined."

## A dedicated symbol for `never`

TypeScript's `never` is overloaded — it means both "uninhabited type" (no values exist) and "this parameter is ignored." These are the same thing in type theory, but at the user level they cause confusion:

- `TypedAction<never, string>` looks like "this action can never be called" to someone unfamiliar with the system
- The actual intent is "this action ignores its input"

A branded symbol would make the intent explicit:

```ts
// Option 1: Branded type alias
declare const IgnoredInput: unique symbol;
type Ignored = never & { [IgnoredInput]: true };

// Option 2: Just a type alias (documentation only, erases to never)
type Ignored = never;
```

Option 1 doesn't work — `never & T` is `never` for any `T`. You can't brand `never`.

Option 2 works as documentation but has no type-level effect. TypeScript will display `never` in hover tooltips, not `Ignored`. This is still valuable in source code (function signatures, doc comments) even if the tooltip doesn't honor it.

### What would actually help

The real problem isn't `never` vs `void` vs a symbol — it's that `pipe()` can't propagate type context backwards through generic arguments. When you write `pipe(get(), someAction)`, TypeScript resolves `get()`'s output type first, then checks if it matches `someAction`'s input. This works. But when `get()` appears in a position where its input type should be inferred from context (e.g., as the second argument in a pipe where the first argument's output is known), the backwards propagation doesn't happen for invariant types.

No choice of input type fixes this. The fix is either:
1. Postfix methods (`.then()`, `.get()`) where `this` provides the type context
2. Explicit type annotations at call sites
3. HOAS patterns where the callback provides the typed tokens

All three are already in use in barnum.
