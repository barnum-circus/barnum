# Enforce `never` output in loop bodies

## Motivation

The `loop` body signature requires output type `never`:

```ts
// Current signature (ast.ts:1362)
export function loop<TBreak = void, TRecur = void>(
  bodyFn: (
    recur: TypedAction<VoidToNull<TRecur>, never>,
    done: TypedAction<VoidToNull<TBreak>, never>,
  ) => Pipeable<VoidToNull<TRecur>, never>,  // ← body must return never
): TypedAction<PipeIn<TRecur>, VoidToNull<TBreak>>
```

This constraint exists because every path in a loop body MUST terminate in either `recur` or `done` — there's no "normal completion." Both `recur` and `done` have output type `never`, so any chain ending in them produces `never`.

**The problem:** this constraint is silently violated when `any` leaks into the body's output type. TypeScript's `any` is assignable to everything, including `never`. The primary source of `any` leakage is `bindInput<TIn>` — when you specify `TIn` explicitly, TypeScript uses the default `TOut = any` instead of inferring from the body return:

```ts
// This compiles WITHOUT error, but is unsound:
loop<string>((recur, done) =>
  bindInput<State>((state) => {
    // body returns TypedAction<any, any>
    // any satisfies never — no error
    return state.then(someAction); // forgot to call recur/done!
  }),
);
```

The user must write `bindInput<State, never>` to get the error. Forgetting `never` produces a silent type hole.

---

## Current state

### Where the constraint exists

Only `loop` requires `never` as the body output:

| Combinator | Body return type | Output constrained to `never`? |
|------------|-----------------|-------------------------------|
| `loop` | `Pipeable<TRecur, never>` | Yes |
| `earlyReturn` | `Pipeable<TIn, TOut>` | No — normal completion allowed |
| `tryCatch` | `Pipeable<TIn, TOut>` | No — normal completion allowed |
| `bindInput` | `Pipeable<any, TOut>` | No — TOut is inferred/defaulted |

### Why `any` leaks through `bindInput`

TypeScript's generic inference is all-or-nothing for a given call. When you write `bindInput<State>(...)`, you've explicitly provided type parameters, so TypeScript uses defaults for unspecified ones rather than inferring:

```ts
// bindInput<TIn, TOut = any>
bindInput<State>((input) => ...)
// TypeScript: TIn = State (explicit), TOut = any (default — NOT inferred)
```

The result: `bindInput<State>(...)` returns `TypedAction<State, any>`. Inside a loop, `TypedAction<State, any>` satisfies `Pipeable<State, never>` because `any extends never` in TypeScript (this is intentional unsoundness in the `any` type).

### The workaround today

From best-practices.md: "When `bindInput` is used inside `loop` where every branch ends in `recur` or `done`, you must specify `TOut = never` explicitly."

```ts
loop<Result, State>((recur, done) =>
  bindInput<State, never>((state) => { ... })
);
```

This works but relies on developer memory. Forgetting `, never` silently passes typechecking.

---

## Proposed change

### Option A: Change `bindInput`'s default from `any` to `unknown`

```ts
// Before
export function bindInput<TIn, TOut = any>(
  body: (input: VarRef<TIn>) => Pipeable<any, TOut>,
): TypedAction<TIn, TOut>

// After
export function bindInput<TIn, TOut = unknown>(
  body: (input: VarRef<TIn>) => Pipeable<any, TOut>,
): TypedAction<TIn, TOut>
```

**Effect:** `bindInput<State>(...)` returns `TypedAction<State, unknown>`. Inside loop, `unknown` is NOT assignable to `never` → compile error. Forces you to write `bindInput<State, never>`.

**Problem:** `unknown` also isn't assignable to anything useful OUTSIDE loops. Every `bindInput<TIn>(...)` call would need explicit TOut or would produce `TypedAction<TIn, unknown>` that doesn't compose downstream. This breaks all existing code.

**Verdict:** Not viable without additional work.

### Option B: Separate overloads for "inferred TOut" vs "explicit TOut"

TypeScript can't partially infer. But we can use a trick: if `bindInput` is called with ONE type param, make the return type use a conditional that forces the user to acknowledge the TOut:

This doesn't work cleanly in TypeScript. Overloads can't distinguish "one type param provided" from "two type params provided" at the type level.

**Verdict:** Not viable with current TypeScript.

### Option C: Accept the status quo, add a lint rule

The constraint IS expressed in the type system (`Pipeable<TRecur, never>`). The leak is through TypeScript's intentional `any` unsoundness. Rather than fighting the type system, add an ESLint rule:

> `bindInput` inside `loop`, `earlyReturn`, or `tryCatch` must specify both type parameters.

This is a project-specific lint rule (add to `@barnum/eslint-plugin`). It catches the forgetting-never case at lint time rather than relying on TypeScript to catch it at type time.

**Verdict:** Viable. Pragmatic. Addresses the actual failure mode.

### Option D: Remove TOut default entirely

```ts
// Before
export function bindInput<TIn, TOut = any>(...): TypedAction<TIn, TOut>

// After — no default
export function bindInput<TIn, TOut>(...): TypedAction<TIn, TOut>
```

**Effect:** Every `bindInput` call MUST specify both type params. No more forgetting.

**Problem:** Outside loop bodies, TOut is genuinely inferable from context and forcing it is ergonomic friction:

```ts
// Currently works:
bindInput<{ artifact: string }>((input) => input.then(verify));

// Would require:
bindInput<{ artifact: string }, { verified: boolean }>((input) => input.then(verify));
```

**Verdict:** Too much friction for the common case.

### Option E: Make loop's body constraint use a branded never

```ts
type LoopTerminal = never & { __loopTerminal: true };

// loop body must return:
() => Pipeable<VoidToNull<TRecur>, LoopTerminal>
```

**Problem:** `never & T = never` in TypeScript. Can't brand `never`.

**Verdict:** Not viable.

### Option F: Constraint-site `any` rejection (allObject pattern)

TypeScript has a pattern to detect `any`:

```ts
type IsAny<T> = 0 extends (1 & T) ? true : false;
```

The naive approach (wrapping the target type) fails — `RejectAny<never>` evaluates to `never` statically, so there's nothing for `any` to be checked against. **However**, the `allObject` inference improvement (`struct.ts`) demonstrates a different technique: put the `any`-detection in the *parameter constraint*, not in what the value is compared to.

#### How `allObject` does it

```ts
type ValidateActions<T extends Record<string, Pipeable<any, any>>> = {
  [K in keyof T]: T[K] extends Pipeable<AllInputs<T>, any>
    ? T[K]
    : Pipeable<AllInputs<T>, any>;
};

export function allObject<const TActions extends Record<string, Pipeable<any, any>>>(
  actions: IsNever<AllInputs<TActions>> extends true
    ? { __error: "..." }
    : TActions & ValidateActions<TActions>,
): ...
```

The key: TypeScript infers `TActions` from the argument, THEN validates the constraint against what was inferred. If validation fails, the parameter type becomes incompatible with what was passed → compile error.

#### Applying to `loop`

The same pattern can reject `any` in the body return type. The body function's return type is inferred by TypeScript first, then we validate it:

```ts
type IsAny<T> = 0 extends (1 & T) ? true : false;

// Extract output type from a Pipeable
type InferOut<T> = T extends Pipeable<any, infer O> ? O : never;

// Validate that the body's output is exactly `never`, rejecting `any`
type ValidateLoopBody<TBody, TRecur> =
  TBody extends Pipeable<VoidToNull<TRecur>, infer TOut>
    ? IsAny<TOut> extends true
      ? Pipeable<VoidToNull<TRecur>, never> & { __error: "loop body must end in recur or done (output must be never, got any)" }
      : TBody
    : Pipeable<VoidToNull<TRecur>, never>;

export function loop<TBreak = void, TRecur = void>(
  bodyFn: (
    recur: TypedAction<VoidToNull<TRecur>, never>,
    done: TypedAction<VoidToNull<TBreak>, never>,
  ) => ValidateLoopBody<Pipeable<VoidToNull<TRecur>, never>, TRecur>,
): TypedAction<PipeIn<TRecur>, VoidToNull<TBreak>>
```

**Problem with this direct approach:** The body return type isn't independently inferred as a generic parameter — it's checked against the declared return type. We'd need to make the body return type itself a generic parameter to inspect it.

#### Viable variant: generic body return

```ts
export function loop<TBreak = void, TRecur = void, TBody extends Pipeable<VoidToNull<TRecur>, any> = Pipeable<VoidToNull<TRecur>, never>>(
  bodyFn: (
    recur: TypedAction<VoidToNull<TRecur>, never>,
    done: TypedAction<VoidToNull<TBreak>, never>,
  ) => IsAny<InferOut<TBody>> extends true
    ? { __error: "loop body output must be never — did you forget to specify TOut on bindInput?" }
    : TBody,
): TypedAction<PipeIn<TRecur>, VoidToNull<TBreak>>
```

Here TypeScript infers `TBody` from the actual return value. If `TBody` has output `any`, `IsAny` triggers and the return type becomes an error object that's incompatible with what was returned → compile error.

**Open question:** Does TypeScript actually infer `TBody` from the callback return when `TBreak` and `TRecur` are explicitly provided? TypeScript's all-or-nothing inference for explicit type params is the same footgun — if the user writes `loop<string>`, TypeScript uses the default for `TBody` rather than inferring. This would require the user to never explicitly specify `TBreak`/`TRecur` (relying entirely on inference), or we'd need a different overload strategy.

#### Alternative: validate at `bindInput`'s constraint site

Rather than validating in `loop`, validate at the `bindInput` call itself. The postfix `.bindInput<TOut>` already requires one explicit type param. We can reject `any` there:

```ts
// On TypedAction's postfix method:
bindInput<TOut>(
  body: (input: VarRef<TCurrentOut>) => IsAny<TOut> extends true
    ? { __error: "bindInput requires explicit TOut type parameter" }
    : Pipeable<any, TOut>,
): TypedAction<TCurrentIn, TOut>
```

This is more targeted — it catches the specific case where `TOut` defaults to `any` because the user forgot to specify it. When `TOut = never` (the correct usage inside loop), `IsAny<never>` is `false`, so the constraint is just `Pipeable<any, never>` as before.

**This is viable** because `TOut` is an explicit type parameter on `.bindInput<TOut>()` — TypeScript uses the provided value (or default) and then checks the constraint. If `TOut` defaults to `any`, `IsAny<any>` fires and produces an incompatible error type.

**Verdict:** Viable. The `allObject` pattern proves this works. Two application points:

1. **On `bindInput`'s body parameter** — reject `any` as TOut at the call site where it matters most
2. **On `loop`'s body return** — requires making body return a generic param (more complex, open question about partial inference)

Approach (1) is strictly better: it catches the problem at the source (`bindInput` defaulting TOut) rather than downstream (`loop` receiving the `any`-poisoned result).

---

## Recommendation

Two viable approaches, complementary:

### Primary: Option F — type-level `any` rejection on `bindInput`

Apply `IsAny<TOut>` in `bindInput`'s body parameter constraint. This catches the problem at the source — when `TOut` defaults to `any`, the constraint becomes an error type, producing a compile error. No lint required.

```ts
bindInput<TOut>(
  body: (input: VarRef<TIn>) => IsAny<TOut> extends true
    ? { __error: "bindInput requires explicit TOut" }
    : Pipeable<any, TOut>,
): TypedAction<TIn, TOut>
```

This mirrors the proven `allObject` / `ValidateActions` pattern. It solves the root cause (TypeScript defaulting `TOut` to `any`) rather than treating symptoms downstream.

**Next step:** prototype this on the postfix `.bindInput<TOut>()` method and verify it produces a clear error when `TOut` is omitted (defaults to `any`) while not interfering when `TOut = never` or a concrete type.

### Secondary: Option C — lint rule (already implemented)

The `barnum/require-type-params` rule enforces explicit type parameters on `loop`, `earlyReturn`, and `bindInput`. This provides defense-in-depth — catching not just `any` leakage but also missing type params that degrade inference in other ways.

The lint rule and the type-level approach are complementary: the type approach catches `any` specifically; the lint rule enforces the broader discipline of explicit type params.

---

## Open questions

1. Is the lint rule worth the implementation cost for a constraint that's already documented?
2. Are there other sources of `any` leakage into loop bodies besides `bindInput`?
3. Should `earlyReturn` also enforce `never` for the body output when ALL paths end in `ret`? (Currently it allows `TOut` — mixed completion is legal.)
