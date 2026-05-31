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

### Option F: Wrapper type that rejects `any`

TypeScript has a pattern to detect `any`:

```ts
type IsAny<T> = 0 extends (1 & T) ? true : false;
type RejectAny<T> = IsAny<T> extends true ? never : T;
```

We could use this in the loop constraint:

```ts
bodyFn: (...) => Pipeable<VoidToNull<TRecur>, RejectAny<never>>
```

But this doesn't help — `RejectAny<never>` evaluates to `never` statically. The issue is that the ACTUAL type passed isn't `never` — it's `any` at the call site, which satisfies `never` before `RejectAny` can inspect it.

**Verdict:** Not viable. TypeScript's `any` bypass happens before conditional type evaluation.

---

## Recommendation

**Option C (lint rule)** is the only viable approach given TypeScript's type system. The type-level constraint already exists and is correct — the gap is that `any` silently satisfies it. A lint rule catches the specific failure mode ("bindInput with one type param inside a loop body") that lets `any` leak through.

The rule:
- Trigger: `bindInput<T>()` (single type param) appears inside the body of `loop`, `earlyReturn`, or any combinator whose body requires `never` output
- Fix: add the second type parameter (almost always `never` in these contexts)
- Rationale: TypeScript can't distinguish "forgot to specify TOut" from "intentionally using default" — lint can

Alternatively, accept this as a known ergonomic wart documented in best practices (which it already is) and don't add tooling.

---

## Open questions

1. Is the lint rule worth the implementation cost for a constraint that's already documented?
2. Are there other sources of `any` leakage into loop bodies besides `bindInput`?
3. Should `earlyReturn` also enforce `never` for the body output when ALL paths end in `ret`? (Currently it allows `TOut` — mixed completion is legal.)
