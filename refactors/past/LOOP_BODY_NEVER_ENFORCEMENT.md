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

The `allObject` inference improvement (`struct.ts`) uses this to validate inferred types at the constraint site:

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

The key: TypeScript infers `TActions` from the argument (always concrete at the call site), THEN validates the constraint.

#### Why this doesn't transfer to `bindInput`

The critical difference: `allObject`'s generic (`TActions`) is **always inferred from a concrete value**. It's never a forwarded generic type variable from an enclosing function scope.

`bindInput`'s `TOut`, on the other hand, is routinely forwarded from enclosing generics:

```ts
// withRetry<TIn, TOut> forwards TOut to bindInput
function withRetry<TIn, TOut>(action: ...): TypedAction<TIn, TOut> {
  return bindInput<TIn, TOut>((input) => ...);
}

// tap<T> forwards T as TOut
function tap<T>(action: ...): TypedAction<T, T> {
  return bindInput<T, T>((input) => ...);
}
```

When `TOut` is a generic type variable, `0 extends 1 & TOut` **defers** — TypeScript can't resolve the conditional. The body return type becomes an unresolved conditional, and the actual return value (a `TypedAction`) can't be proven assignable to it.

This is fundamentally different from the case where `TOut = any` (the default). TypeScript cannot distinguish "TOut is `any` because it defaulted" from "TOut is an unresolved generic type variable" — both defer the conditional at the definition site.

**Verdict:** Not viable. The `allObject` pattern only works when the generic is inferred from a concrete argument. `bindInput`'s `TOut` can be a forwarded generic, which defers all conditional type checks.

---

## Open questions

1. Is the lint rule worth the implementation cost for a constraint that's already documented?
2. Are there other sources of `any` leakage into loop bodies besides `bindInput`?
3. Should `earlyReturn` also enforce `never` for the body output when ALL paths end in `ret`? (Currently it allows `TOut` — mixed completion is legal.)
