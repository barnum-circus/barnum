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

## Redundant `drop` elimination

With `drop` typed honestly as `void` output, several `chain(drop, X)` patterns become unnecessary. Full audit of all `drop` usage in the codebase:

### Redundant — remove after void change

**`chain(drop, constant(true/false))` → `constant(true/false)`** (8 call sites in `builtins.ts`)

`constant` has input `any` — it ignores whatever it receives. The `drop` is a no-op.

- `builtins.ts:498-499` — `Option.isSome` / `Option.isNone`
- `builtins.ts:506-507` — `Option.isNone` / `Option.isSome` (inverted)
- `builtins.ts:673-674` — `Result.isOk` / `Result.isErr`
- `builtins.ts:681-682` — `Result.isErr` / `Result.isOk` (inverted)

**`chain(drop, defaultAction)` in `Option.unwrapOr` → `defaultAction`** (1 call site in `builtins.ts:459`)

The None branch payload is already `void`/`null` (via VoidToNull). Discarding null to produce null is a no-op. `defaultAction` can accept the None payload directly. Requires `defaultAction: Pipeable<never, T>` → `Pipeable<void, T>`.

**`mapErr(drop)` type hack** (1 call site in `demos/retry-on-error/run.ts:41`)

`stepA.mapErr(drop).unwrapOr(done)` — erases error type to `never` so `done` fits. With covariant output (from `CONSOLIDATE_PHANTOM_FIELDS.md`), `done` works in `Pipeable` slots directly. The `mapErr(drop)` is unnecessary.

### Still needed — discarding a real value

**`chain(drop, other)` in `Result.and`** (`builtins.ts:590`)

Ok branch payload is `TValue`, not void. `drop` genuinely discards the Ok value before calling `other`.

**`drop.tag("None")`** (3 call sites in `builtins.ts`)

Discards the branch payload (e.g., `TError` in `Result.toOption`) before wrapping as `{ kind: "None", value: null }`. Without `drop`, the payload would become the None value — wrong.

**`pipe(drop, body(input))` in `bindInput`** (`bind.ts:160`)

Discards the bind result so the body pipeline starts fresh. Input is accessed through VarRef.

**`.drop().then(X)` in demos** — discards handler output before continuing with a new value:

- `babysit-prs/run.ts:46` — `fixIssues.drop().then(prNumber)`
- `babysit-prs/run.ts:49` — `landPR.drop().then(Option.none())`
- `identify-and-address-refactors/handlers/refactor.ts:305-307` — `applyFeedback.drop().then(...).then(recur)`

**`.drop()` for pipeline sequencing** — discards handler output so the next pipe step receives void. The value is genuinely discarded, not already void:

- `retry-on-error/run.ts:32` — `stepB.unwrapOr(throwError).drop()`
- `retry-on-error/run.ts:41,47` — `unwrapOr(done).drop()`, `unwrapOr(throwError).drop()`
- `identify-and-address-refactors/handlers/refactor.ts:299-300,310,313` — `implement.drop()`, `typeCheckFix.drop()`, `commit.drop()`
- `convert-folder-to-ts/run.ts:27` — handler output discarded

**`drop` as branch case handler** — discards variant payload entirely:

- `babysit-prs/run.ts:50` — `Landed: drop`
- `identify-and-address-refactors/handlers/refactor.ts:308` — `Approved: drop`

**`forEach(fix).drop().then(recur)` pattern** — discards forEach output before recursing:

- `identify-and-address-refactors/handlers/type-check-fix.ts:146`
- `convert-folder-to-ts/handlers/type-check-fix.ts:147`

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
