# Void vs Never Semantics Cleanup

## Motivation

`never` is overloaded in barnum. It currently means two different things:

1. **Genuinely unreachable** — the pipeline halts or diverges. Execution does not continue past this point. Examples: `throwError` (performs a restart effect), `recur`/`done` in `loop` (perform restart effects), `RestartPerform`.

2. **No useful value** — the action completes and the pipeline continues, but the output is meaningless (null at runtime). Examples: `drop` (discards input, returns null), `sleep` (waits, returns null), `dropResult` (runs an action for side effects, returns null).

These are semantically distinct. A `TypedAction<X, never>` should mean "placing this in a pipeline terminates that branch of execution." But `drop` doesn't terminate anything — it's a normal pipeline step that produces null. The `never` output type forces `as any` casts when composing `drop` with subsequent steps, because `never` as an invariant phantom type doesn't unify with downstream input types the way a concrete type would.

The fix: use `void` (mapped to `null` at runtime via the existing `VoidToNull` machinery) for "no useful value," and reserve `never` for genuinely unreachable code paths.

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

### 3. `dropResult(action)`: `TypedAction<TInput, never>` → `TypedAction<TInput, void>`

**File:** `libs/barnum/src/builtins.ts:209-212`

```ts
// Before
export function dropResult<TInput, TOutput>(
  action: Pipeable<TInput, TOutput>,
): TypedAction<TInput, never> {
  return chain(action, drop) as TypedAction<TInput, never>;
}

// After
export function dropResult<TInput, TOutput>(
  action: Pipeable<TInput, TOutput>,
): TypedAction<TInput, void> {
  return chain(action, drop) as TypedAction<TInput, void>;
}
```

`dropResult` is `chain(action, drop)`. Once `drop` returns `void`, the `as` cast here might even become unnecessary (depends on how invariant phantom types resolve — see open questions).

### 4. `.drop()` postfix method: return `TypedAction<In, never>` → `TypedAction<In, void>`

**File:** `libs/barnum/src/ast.ts:195`

```ts
// Before
drop(): TypedAction<In, never, Refs>;

// After
drop(): TypedAction<In, void, Refs>;
```

### 5. `HandlerOutput<void>`: `never` → `void`

**File:** `libs/barnum/src/handler.ts:98`

```ts
// Before
type HandlerOutput<TOutput> = [TOutput] extends [void] ? never : TOutput;

// After — remove the type entirely, or change to:
type HandlerOutput<TOutput> = [TOutput] extends [void] ? void : TOutput;
```

Currently, a handler returning `Promise<void>` produces `TypedAction<TIn, never>`, meaning "this handler never returns." That's wrong — the handler does return, it just returns nothing useful. With `void` output, fire-and-forget handlers produce `TypedAction<TIn, void>` (null at runtime), which is composable without casts.

The original JSDoc says this exists so fire-and-forget handlers "compose without `.drop()`". With `void` output they still compose — `void` (null) flows into any downstream step that accepts its input type. The difference is that `void` is an honest type rather than a lie (`never` promises the handler never returns, but it does).

### 6. Unchanged: `throwError`, `recur`, `done`

These stay `TypedAction<..., never>`. They perform `RestartPerform` effects that transfer control to a restart handler — execution genuinely does not continue past them in the current branch.

- `throwError` in `tryCatch`: `TypedAction<TError, never>` — correct.
- `recur` token in `recur()`: `TypedAction<TIn, never>` — correct.
- `recur`/`done` tokens in `loop()`: `TypedAction<TIn, never>` / `TypedAction<VoidToNull<TBreak>, never>` — correct.

---

## Impact on `as any` casts

Three call sites in `builtins.ts` use `drop as any` today:

```ts
// builtins.ts:645 — Result.toOption
Err: chain(drop as any, tag("None")),

// builtins.ts:655 — Result.toOptionErr
Ok: chain(drop as any, tag("None")),

// builtins.ts:671 — Result.transpose
None: chain(drop as any, tag("None")),
```

The `as any` is needed because `drop` is `TypedAction<any, never>`, `tag("None")` expects a specific input type, and `never` as an invariant output doesn't unify with that input. With `drop` typed as `TypedAction<any, void>`, the output is `void` (null). Whether this eliminates the cast depends on whether `void` satisfies the invariant phantom constraints on `tag("None")`'s input — the `None` variant payload is `void`, and `VoidToNull<void>` is `null`, so the chain would be `void → null → tag("None")`. This needs verification (see open questions).

Several other call sites use `chain(drop, ...)` without `as any`:

```ts
// builtins.ts:469, 508-509, 516-517, 600, 683-684, 691-692
chain(drop, constant(true))
chain(drop, constant(false))
chain(drop, defaultAction)
chain(drop, other)
```

These already work because `chain` infers the intermediate type, and `constant`'s input is `any`. After the change, `chain<any, void, boolean>(drop, constant(true))` — still fine.

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

## Open questions

1. **Does `void` interact correctly with invariant phantom types?** `TypedAction` uses invariant phantom fields (`__out` covariant + `__out_contra` contravariant). With `Out = void`, these become `__out?: () => void` and `__out_contra?: (output: void) => void`. TypeScript's `void` in these positions should behave as a concrete type (unlike `never`, which is the bottom type). The question is whether `chain(drop, tag("None"))` typechecks without `as any` when `drop`'s output is `void` and `tag("None")`'s input expects the None variant's payload type. This needs to be verified against the actual phantom type machinery — try the change and read the compiler errors.

2. **Should `constant(null)` replace `drop`?** After this change, `drop` is semantically `constant(null)` with an `any` input. They differ only in that `drop` accepts any input while `constant(null)` is `TypedAction<any, null>`. If `void` maps to `null` at runtime, these are equivalent. `drop` is still worth keeping as a named concept — "discard the value" reads better than "replace with null" — but it's worth noting they converge.

3. **Test updates.** The type test in `libs/barnum/tests/types.test.ts:923` explicitly asserts `throwError` is `TypedAction<TError, never>`. No change needed there. But any tests that assert `drop` is `TypedAction<any, never>` or `sleep` is `TypedAction<any, never>` will need updating. The `.drop()` postfix tests (line 1167, 1175) assert `TypedAction<never, never>` — these become `TypedAction<never, void>`.
