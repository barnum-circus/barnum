# TypeScript Inference Quirks for Barnum

Reference doc capturing every TypeScript type system behavior that affects
Barnum's DSL design. Not a refactor proposal — a knowledge base.

## Invariance via paired phantom fields

TypeScript has no `Invariant<T>`. Barnum fakes it by pairing a covariant
field with a contravariant field:

```typescript
type Pipeable<In, Out> = Action & {
  __phantom_in?: (input: In) => void;  // contravariant
  __phantom_out?: () => Out;           // covariant
  __phantom_out_check?: (output: Out) => void;  // contravariant (pairs with __phantom_out)
  __in?: In;                           // covariant (pairs with __phantom_in)
};
```

Each pair (covariant + contravariant) produces invariance. Both directions
must agree, so `Pipeable<string, X>` is not assignable to `Pipeable<number, X>`.

This is critical because handler data crosses serialization boundaries —
extra or missing fields are runtime errors.

## `any` defeats invariance

`any` is assignable to everything AND everything is assignable to `any`.
It passes both covariant and contravariant checks simultaneously.

- `Pipeable<any, never>` is assignable to `Pipeable<X, never>` for ANY X.
- This is why `drop<any>()` works as a universal bridge — it introduces
  `any` which bypasses all invariance checks.
- Corollary: `= any` defaults on type params that can't be inferred
  silently erase all type checking downstream.

## HOAS callback param types cannot be inferred from usage

TypeScript infers generic params from **concrete argument types** — values
the caller passes in. It matches the argument's type against the expected
parameter type and extracts generics. For callbacks, it infers from the
callback's RETURN type (which is a concrete expression TS can synthesize
a type for), not from how callback PARAMETERS are used inside the body.

```typescript
loop((recur, done) => body)
```

TS cannot infer `TIn` or `TBreak` from how `recur` and `done` are used
inside `body`. It can only infer from `body`'s return type.

The reason is about **inference sites**. TS needs a concrete type to
match against a generic parameter. Direct arguments and callback return
expressions provide inference sites — they're values TS synthesizes
types for. Callback parameters don't — they're values TS *constructs*
(to pass to the callback), not values TS *analyzes*. `TBreak` in
`done: TypedAction<TBreak, never>` has zero inference sites in `loop`'s
signature, so TS falls back to the default or `unknown`.

Inferring from usage would require TS to observe that `done` is used in
a pipeline position expecting `TypedAction<number, never>`, then
propagate `number` back to `TBreak`. This is checking-direction
inference (type flows inward from usage site to declaration). TS only
does synthesis-direction inference (type flows outward from expression
to consumer). Flow had a more aggressive algorithm that could handle
the checking direction, which is why `useState` inference worked
differently in Flow vs TypeScript.

**Callback return type inference DOES work reliably.** For example,
`earlyReturn((ret) => loop<X, Y>(...))` correctly infers TIn and TOut
from the loop's return type, because the return expression is a
concrete value TS can synthesize a type for.

### Why tryCatch doesn't have this problem

```typescript
function tryCatch<TIn, TOut, TError>(
  body: (throwError: TypedAction<TError, never>) => Pipeable<TIn, TOut>,
  recovery: Pipeable<TError, TOut>,
): TypedAction<TIn, TOut>
```

`recovery` is a **direct argument** — a concrete value the caller
passes. TS synthesizes its type, matches it against
`Pipeable<TError, TOut>`, and extracts `TError` and `TOut`. Then it
uses `TError` to type `throwError` inside the callback.

The inference flow: concrete argument `recovery` provides `TError` →
TS constructs `throwError: TypedAction<TError, never>` → callback
body is type-checked with `throwError` fully typed.

This is the escape hatch for HOAS inference: if the generic appears
in both a callback parameter AND a direct argument, TS infers it from
the direct argument. The callback parameter gets it for free.

`loop` has no equivalent escape hatch — there's no second argument
from which to infer `TIn` or `TBreak`.

### Design rule

HOAS combinator type params split into two categories:

1. **Token types** (callback parameter types): cannot be inferred from
   usage. Require either an explicit annotation, a direct-argument
   inference site (like tryCatch's `recovery`), or a semantically
   correct default.
2. **Body types** (callback return types): CAN be inferred (synthesis
   direction). Safe to give `= any` defaults as a fallback.

Current signatures:

| Combinator    | Required (token types)  | Defaulted (body types)           | Escape hatch                     |
|---------------|------------------------|----------------------------------|----------------------------------|
| `loop`        | `TIn`, `TBreak`        | `TRefs = never`                  | none — must annotate             |
| `earlyReturn` | —                      | `TEarlyReturn = never`, `TIn = any`, `TOut = any` | none, but defaults work          |
| `recur`       | `TIn`                  | `TOut = any`                     | none — must annotate             |
| `tryCatch`    | —                      | all inferred                     | `recovery` arg provides `TError` |

## No partial type argument inference

Once you provide ANY explicit type arg, inference is disabled for ALL
remaining params. They fall back to defaults.

```typescript
earlyReturn<string>(...)
// TEarlyReturn = string (explicit)
// TIn = any (DEFAULT, not inferred)
// TOut = any (DEFAULT, not inferred)
// Output: TypedAction<any, string | any> = TypedAction<any, any>
// TEarlyReturn is swallowed by any.
```

TypeScript has no syntax for "infer this param, I'll provide that one."

## `never` is bottom, `unknown` is top

- `never` extends everything. `unknown` accepts everything.
- `(input: X) => void` is assignable to `(input: never) => void` for
  all X (contravariant: `never extends X` is always true).
- A phantom `__phantom_in?: (input: never) => void` imposes no constraint
  when matching `CaseHandler<TError, TValue>` — it says
  TError ⊇ never, which is trivially true.
- This is why `TypedAction<never, never>` is a safe default for
  earlyReturn/throw tokens — it never overconstrains inference.

## `CaseHandler` uses deliberately relaxed variance

`CaseHandler` has only `__phantom_in` (contravariant) and `__phantom_out`
(covariant). It omits `__in` and `__phantom_out_check`.

This means:

- **Contravariant input only:** `TypedAction<unknown, X>` (accepts
  anything) is assignable to `CaseHandler<SpecificType, X>` because
  `(input: unknown) => void` is assignable to `(input: SpecificType) => void`.
- **Covariant output only:** `TypedAction<X, never>` is assignable to
  `CaseHandler<X, TValue>` for any TValue, because `() => never` is
  assignable to `() => TValue`.

Full invariance (via `Pipeable`) would reject both of these.

## `unwrapOr` infers TError from both `this` and `defaultAction`

The `this` parameter provides `Result<TValue, TError>`. The
`defaultAction: CaseHandler<TError, TValue>` also constrains TError
via `__phantom_in`.

If the defaultAction's input type disagrees with the Result's error type,
you get a conflict:

```typescript
// earlyReturn<string> token: TypedAction<string, never>
// After mapErr(drop()): Result<string, never>
stepC.mapErr(drop()).unwrapOr(earlyReturn)
// TS infers TError = string from earlyReturn's __phantom_in
// But this context has Result<string, never> → TError = never
// Conflict: Result<string, never> not assignable to Result<string, string>
```

Tokens with `never` input don't have this problem — they impose no
constraint on TError.

## Discriminated unions break optional phantom field inference

`Action` is a DU of 8 variants. When TS infers a type param from an
optional field on a DU member, `__refs?: never` collapses to `undefined`,
and inference falls back to the constraint bound `string`.

Fix: box the phantom — `__refs?: { _brand: Refs }`. The wrapper
`{ _brand: never }` is structurally distinct from `undefined`, so
inference resolves correctly.

This ONLY manifests with the real 8-variant Action union. A simple
`{ kind: string }` infers fine. Union distribution changes how TS
resolves optional fields.

## Method signatures cause recursive inference chaos

`TypedAction` has methods (`.then()`, `.branch()`, etc.) whose signatures
reference `TypedAction` recursively. These participate in assignability
checks and can confuse pipe overload resolution.

`Pipeable` strips all methods, keeping only phantom fields. Pipe overloads
use `Pipeable` so inference is driven by phantom fields alone —
predictable covariant/contravariant resolution.

`TypedAction` is assignable to `Pipeable` because `Pipeable` requires a
subset of properties.

## `WorkflowAction` checks `__in?: void`, not `__phantom_in`

Workflows start with no input. `void` for the covariant `__in` accepts
both `any` (combinators ignoring input) and `never` (handlers with no
params).

The contravariant `__phantom_in` is NOT checked — with `void` it would
accept everything, making the check vacuous.

## `= any` vs no default vs `= unknown`

- **`= any`**: inference failure → `any` → silently erases type checking.
  Dangerous for HOAS params that can never be inferred.
- **No default**: inference failure → `unknown` (implicit fallback for
  unconstrained params). `unknown` fails invariance checks, surfacing
  the error.
- For params that CAN be inferred (callback return types), `= any` is a
  harmless safety net — inference succeeds in practice, the default
  rarely fires.
- For params that CANNOT be inferred (HOAS token types), `= any` is a
  bug factory.

## `drop<any>()` as a type bridge

When a loop body starts with `never`-input handlers but the loop's TIn
is `any`, invariance rejects the mismatch: `Pipeable<never, X>` is not
assignable to `Pipeable<any, X>`.

`drop<any>()` bridges this: it's `TypedAction<any, never>`, accepting
`any` (matching the loop) and outputting `never` (matching the handlers).

However, the correct fix is usually to set TIn to `never` when the loop
genuinely receives no meaningful input. Then the body's `never` input
matches directly and `drop<any>()` is unnecessary.

## Function param contravariance, return type covariance

- `(x: Dog) => void` is NOT a subtype of `(x: Animal) => void`
  (params are contravariant).
- `() => Dog` IS a subtype of `() => Animal` (returns are covariant).
- This is the foundation: `__phantom_in?: (input: T) => void` gives
  contravariance, `__phantom_out?: () => T` gives covariance.

## Overloads for HOAS bridging: attempted and abandoned

We tried overloads where a `Pipeable<never, ...>` body maps to
`TypedAction<any, ...>` output. Problems:

1. Implementation signature compatibility — invariance makes the
   implementation signature hard to write.
2. Even when matching, TS still infers wrong types for body internals.
   Removing `drop()` on `logError` still gave `any` inference for the
   loop body.
3. All-or-nothing: overloads don't fix the fundamental issue that TS
   can't infer HOAS token types from callback body usage.
