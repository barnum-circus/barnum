# Phase 5: Advanced Patterns (RAII, Timeout)

## Goal

Two features that stress-test Handle/Perform in different ways. Each is TypeScript sugar over the existing substrate. No scheduler changes — only new handler DAGs and TS surface functions.

## Prerequisites

Phase 1 (Substrate), Phase 3 (TryCatch — for the discard path and teardown).

## See also

- [EFFECTS_RACE.md](./EFFECTS_RACE.md) — extracted to its own doc.

## RAII (Bracket effect)

### The problem

Resources (worktrees, temp files, database connections) must be cleaned up when their scope exits, regardless of success or failure. Currently handled by `withResource`, which couples creation, usage, and disposal into one combinator.

### The solution

A Bracket effect type with automatic scope-exit cleanup. Separated from variable binding (ReadVar is unrestricted, Bracket is affine).

```
Effect: Bracket
Handler behavior:
  On scope entry: evaluate acquire action, store resource.
  On scope exit (success or error): run dispose on stored resource.
```

The Handle frame for Bracket tracks acquired resources. When the Handle frame exits — whether the body completed or an error propagated — it runs dispose for each tracked resource, in reverse acquisition order.

### How it compiles

```ts
// User writes:
declare([pipe(deriveBranch, createWorktree)], ([wt]) => body)
// Where createWorktree has dispose metadata.

// Builder gensyms two EffectIds: bracketEffect, readVarEffect.
// Compiles to nested handlers:
Chain(
  pipe(deriveBranch, createWorktree),
  Handle(bracketEffect, bracketHandler,
    Handle(readVarEffect, readVarHandler,
      body   // uses Perform(readVarEffect) to access the worktree
    )
  )
)
```

The Bracket Handle is the outer scope. The ReadVar Handle is inner. On exit, Bracket runs dispose. ReadVar has already been cleaned up (inner scope exits first).

### Dispose execution

Dispose is itself a handler Invoke (calls TypeScript). The scheduler pushes dispose tasks to pending dispatches. Dispose failures are tracked as suppressed errors alongside the primary result/error.

### What this replaces

`withResource({ create, action, dispose })` is subsumed. The user doesn't specify dispose at the call site — it's declared on the handler.

## Timeout

### The problem

An action must complete within a duration. If it doesn't, the losers are cancelled and the caller must handle both outcomes.

### The solution

`withTimeout` returns `Result<TOut, void>` — a standard discriminated union. The `Err` variant is `void` because there's nothing meaningful to report beyond "it didn't finish."

`Result<TValue, TError>` is a general-purpose type alongside `Option<T>` and `LoopResult<TContinue, TBreak>`:

```ts
type ResultDef<TValue, TError> = { Ok: TValue; Err: TError };
type Result<TValue, TError> = TaggedUnion<ResultDef<TValue, TError>>;

function withTimeout<TIn, TOut>(
  duration: number,
  body: Pipeable<TIn, TOut>,
): TypedAction<TIn, Result<TOut, void>>
```

### Usage

```ts
pipe(
  withTimeout(5000, longRunningAction),
).branch({
  Ok: processResult,
  Err: pipe(drop(), constant("timed out")),
})
```

The caller branches on `Ok`/`Err`. No exceptions, no implicit error paths.

### How it compiles

`withTimeout` is sugar over `race`. Both branches tag their output into the `Result` union:

```ts
withTimeout(5000, body)

// Compiles to:
race(
  pipe(body, tag<ResultDef<TOut, void>, "Ok">("Ok")),
  pipe(timer(5000), tag<ResultDef<TOut, void>, "Err">("Err")),
)
```

Both branches produce `Result<TOut, void>`, satisfying race's homogeneous output requirement. The first branch to complete wins; the loser is cancelled via race's teardown semantics.

### Timer action

`timer(duration)` is an Invoke that the external driver resolves after `duration` milliseconds. The handler produces `void` (no payload). The driver protocol needs a timer registration mechanism — the scheduler emits a timer request, the driver calls back when it fires.

```ts
function timer(duration: number): TypedAction<never, void>
```

Input is `never` — the timer doesn't consume pipeline data. It's a side-channel action resolved by the runtime.

## Test strategy

### RAII tests

1. Resource acquired and disposed on normal exit.
2. Resource disposed on error exit (body throws).
3. Multiple resources disposed in reverse order.
4. Nested Bracket scopes: inner disposes before outer.
5. Dispose failure: primary result still delivered, dispose error attached as suppressed.
6. Bracket + ReadVar interaction: variable readable throughout body, disposed after body exits.

### Timeout tests

1. Body completes before timeout. Timer cancelled. Result is `{ kind: "Ok", value: bodyOutput }`.
2. Timeout fires before body completes. Body cancelled. Result is `{ kind: "Err", value: undefined }`.
3. Timeout result branched: `Ok` case processes output, `Err` case produces fallback.
4. Timeout with RAII: body acquires resource, timeout fires, resource disposed, result is `Err`.

## Deliverables

1. Bracket handler DAG (track resources in state, dispose on exit via `StateUpdate::Updated`)
2. Bracket Handle frame logic (state-based resource tracking, dispose on scope exit)
3. `withTimeout()` TypeScript function (depends on `race()` from EFFECTS_RACE.md)
4. Timer action (Invoke that external driver resolves after duration)
5. Tests per above
6. Demo: add timeout and error handling to an existing demo workflow
