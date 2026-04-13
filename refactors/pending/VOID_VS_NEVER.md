# Void vs Never Semantics Cleanup

## Motivation

`never` is overloaded in barnum. It currently means two different things:

1. **Genuinely unreachable** — the pipeline halts or diverges. Execution does not continue past this point. Examples: `throwError` (performs a restart effect), `recur`/`done` in `loop` (perform restart effects), `RestartPerform`.

2. **No useful value** — the action completes and the pipeline continues, but the output is meaningless (null at runtime). Examples: `drop` (discards input, returns null), `sleep` (waits, returns null), postfix `.drop()` (runs an action for side effects, returns null).

These are semantically distinct. A `TypedAction<X, never>` should mean "placing this in a pipeline terminates that branch of execution." But `drop` doesn't terminate anything — it's a normal pipeline step that produces null. The `never` output type forces `as any` casts when composing `drop` with subsequent steps, because `never` as an invariant phantom type doesn't unify with downstream input types the way a concrete type would.

The fix: use `void` (mapped to `null` at runtime via the existing `VoidToNull` machinery) for "no useful value," and reserve `never` for genuinely unreachable code paths.

**Dependency on `CONSOLIDATE_PHANTOM_FIELDS.md`:** Several issues identified in the investigation below are caused by output invariance, not by `never` vs `void` specifically. The output covariance change proposed in `CONSOLIDATE_PHANTOM_FIELDS.md` (remove `__out_contra`, make output covariant-only) resolves them:

- **`chain(drop, tag("None"))` casts:** With invariant output, `void` doesn't unify with `tag("None")`'s generic input. With covariant output, `void extends T` passes, so the cast may become unnecessary. (Postfix `drop.tag("None")` already avoids the cast regardless.)
- **`mapErr(drop)` producing `Result<V, void>` instead of `Result<V, never>`:** This concern disappears because `mapErr(drop)` was a type-level hack to erase error types via `never`'s bottom-type behavior. It's not a real use case — if you want to handle the error, use `unwrapOr`. The `void` error type is honest.
- **`Option.unwrapOr` / `Result.and` signature changes (`Pipeable<never, T>` → `Pipeable<void, T>`):** With covariant output, `void extends T` passes naturally, so these signatures may not need changing at all. Needs verification after the covariance change lands.

---

## Proposed rule

- **`never`** = genuinely unreachable. The pipeline halts, diverges, or performs a non-local control flow effect. Nothing runs after this action in the current pipeline branch.
- **`void`** (→ `null` at runtime via `VoidToNull`) = no useful value. The pipeline continues normally. The output is `null` and can be observed, passed to the next step, etc.

---

## Changes

### 1. `drop`: `TypedAction<any, never>` → `TypedAction<any, void>`

**File:** `libs/barnum/src/builtins.ts:100`

```ts
// Before
export const drop: TypedAction<any, never> = typedAction({...});

// After
export const drop: TypedAction<any, void> = typedAction({...});
```

`drop` discards its input and returns `null` at runtime. It does not halt the pipeline. With `void` output, `chain(drop, constant(true))` typechecks without casts — `void` (null) flows into `constant`'s `any` input naturally.

### 2. `sleep(ms)`: `TypedAction<any, never>` → `TypedAction<any, void>`

**File:** `libs/barnum/src/race.ts:82`

```ts
// Before
export function sleep(ms: number): TypedAction<any, never> {...}

// After
export function sleep(ms: number): TypedAction<any, void> {...}
```

Same reasoning as `drop`. Sleep completes and the pipeline continues with null.

### 3. `.drop()` postfix method: return `TypedAction<In, never>` → `TypedAction<In, void>`

**File:** `libs/barnum/src/ast.ts`

```ts
// Before
drop(): TypedAction<In, never>;

// After
drop(): TypedAction<In, void>;
```

(`dropResult` has been removed — use postfix `.drop()` or `chain(action, drop)` instead.)

### 4. `HandlerOutput<void>`: `never` → `void`

**File:** `libs/barnum/src/handler.ts:98`

```ts
// Before
type HandlerOutput<TOutput> = [TOutput] extends [void] ? never : TOutput;

// After — remove the type entirely, or change to:
type HandlerOutput<TOutput> = [TOutput] extends [void] ? void : TOutput;
```

Currently, a handler returning `Promise<void>` produces `TypedAction<TIn, never>`, meaning "this handler never returns." That's wrong — the handler does return, it just returns nothing useful. With `void` output, fire-and-forget handlers produce `TypedAction<TIn, void>` (null at runtime), which is composable without casts.

The original JSDoc says this exists so fire-and-forget handlers "compose without `.drop()`". With `void` output they still compose — `void` (null) flows into any downstream step that accepts its input type. The difference is that `void` is an honest type rather than a lie (`never` promises the handler never returns, but it does).

### 5. Unchanged: `throwError`, `recur`, `done`

These stay `TypedAction<..., never>`. They perform `RestartPerform` effects that transfer control to a restart handler — execution genuinely does not continue past them in the current branch.

- `throwError` in `tryCatch`: `TypedAction<TError, never>` — correct.
- `recur` token in `recur()`: `TypedAction<TIn, never>` — correct.
- `recur`/`done` tokens in `loop()`: `TypedAction<TIn, never>` / `TypedAction<VoidToNull<TBreak>, never>` — correct.

---

## Impact on `as any` casts

Three call sites in `builtins.ts` previously used `chain(drop as any, tag("None"))`. These have already been replaced with postfix `drop.tag("None")`, which avoids the cast entirely (the `as any` is internal to the postfix method implementation).

Several call sites use `chain(drop, ...)` without `as any`:

```ts
chain(drop, constant(true))
chain(drop, constant(false))
chain(drop, defaultAction)
chain(drop, other)
```

These already work because `constant`'s input is `any`. After the change to `void` output, `chain<any, void, boolean>(drop, constant(true))` — still fine.

---

## VoidToNull consistency

The `VoidToNull` utility type already maps `void → null` in tagged union variant payloads:

```ts
type VoidToNull<T> = 0 extends 1 & T ? T : [T] extends [never] ? never : [T] extends [void] ? null : T;
```

After this change, `void` consistently means "null at runtime" throughout the framework:
- Tagged union variants: `{ None: void }` → runtime value `{ kind: "None", value: null }` (already works via `VoidToNull`)
- Pipeline actions: `TypedAction<X, void>` → runtime output is `null` (new, consistent with above)
- Handler output: `Promise<void>` handler → `TypedAction<X, void>` → runtime output is `null` (new)

---

## Investigation results

Tested by changing `drop` from `TypedAction<any, never>` to `TypedAction<any, void>` and running the typechecker.

### What works

- `chain(drop, constant(true))` and `chain(drop, constant(false))` typecheck cleanly — no casts needed. `constant`'s `any` input absorbs `void` output.
- Two library signature changes required: `Option.unwrapOr` and `Result.and` parameter types change from `Pipeable<never, T>` to `Pipeable<void, T>` (contravariance: `void` is not assignable to `never`).

### What doesn't work

- **`chain(drop, tag("None"))` still needs `as any`.** Different error from the `never` case, but same fundamental problem: `tag("None")` has an unresolved generic input type, and the invariant output encoding of `drop` prevents inference.
- **`mapErr(drop)` pattern breaks.** With `drop: TypedAction<any, void>`, `mapErr(drop)` produces `Result<string, void>` instead of `Result<string, never>`. The ergonomic pattern of erasing error types via `mapErr(drop)` to collapse them to `never` no longer works — `void` doesn't collapse unions the way `never` does.

### Verdict

With the current invariant output, this change has friction — casts don't go away and `mapErr(drop)` breaks. However, the output covariance change in `CONSOLIDATE_PHANTOM_FIELDS.md` resolves most issues. The recommended order is:

1. Land output covariance first (remove `__out_contra`)
2. Then change `drop`/`sleep` to `void` output — most friction disappears

The `mapErr(drop)` pattern is not a real use case — it was type gymnastics to erase error types via `never`'s bottom-type behavior. With covariant output, `throwError` and `done` work directly in `Pipeable` slots without `CaseHandler`, making the error-erasure hack unnecessary.
