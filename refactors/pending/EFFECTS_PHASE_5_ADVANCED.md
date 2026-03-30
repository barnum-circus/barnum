# Phase 5: Advanced Patterns (RAII, Race, Timeout)

## Goal

Three features that stress-test Handle/Perform in different ways. Each is TypeScript sugar over the existing substrate. No scheduler changes — only new handler DAGs and TS surface functions.

## Prerequisites

Phase 1 (Substrate), Phase 3 (TryCatch — for the discard path and teardown).

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

## Race

### The problem

Run multiple actions concurrently. The first to complete wins. The losers are cancelled.

### The solution

Race is Handle + Parallel + cancellation. Not a new AST node.

```ts
// User writes:
race(actionA, actionB)

// Builder gensyms a fresh EffectId: raceEffect.
// Handler DAG: pipe(pick("payload"), tag("Discard"))
// Compiles to:
Handle(raceEffect, raceHandler,
  Parallel(
    Chain(actionA, Perform(raceEffect)),
    Chain(actionB, Perform(raceEffect)),
  )
)
```

The handler receives `{ payload: firstResult, state: ... }`, extracts the payload, and produces Discard. The Handle frame tears down the body (including the un-completed Parallel branch) and exits with the first result.

### Cancellation semantics

When the Handle exits with a live Parallel frame below it, teardown must:
1. Cancel pending external tasks in the losing branch.
2. Run Bracket dispose for any resources the losing branch acquired.
3. Remove all frames from the slab.

This uses the same `teardown_body` from Phase 1. Race doesn't add new teardown logic — it exercises existing teardown under Parallel.

### Type safety

`race(a, b)` requires both branches to produce the same output type (since either could win). The TypeScript function enforces this:

```ts
function race<TIn, TOut>(
  ...actions: Pipeable<TIn, TOut>[]
): TypedAction<TIn, TOut>
```

## Timeout

### The problem

An action must complete within a duration. If it doesn't, it's cancelled and an error is produced.

### The solution

Timeout combines Handle with an external timer.

```ts
// User writes:
withTimeout(duration, body)

// Compiles to race between body and timer:
race(body, timer(duration))
// Where timer(duration) is an Invoke resolved by the external driver.
```

Or more precisely, timeout is race between the body and a timer action. The timer action is an Invoke that the external driver resolves after the duration. If the timer wins, the body is cancelled (via Race's semantics). If the body wins, the timer is cancelled.

Alternative: Timeout as a dedicated effect where the external driver sends a cancellation signal:

```
Effect: RegisterTimeout(duration)
Handler behavior:
  On entry: emit timeout registration to external driver.
  On body completion: cancel the timer.
  On timer expiry: driver calls cancel_frame, forcing a Throw(TimeoutError).
```

The Race-based approach is simpler and doesn't require new driver protocol. Recommend Race-based.

## Test strategy

### RAII tests

1. Resource acquired and disposed on normal exit.
2. Resource disposed on error exit (body throws).
3. Multiple resources disposed in reverse order.
4. Nested Bracket scopes: inner disposes before outer.
5. Dispose failure: primary result still delivered, dispose error attached as suppressed.
6. Bracket + ReadVar interaction: variable readable throughout body, disposed after body exits.

### Race tests

1. Two branches, first completes. Second is cancelled.
2. Two branches, second completes. First is cancelled.
3. Losing branch has pending external task. Task is cancelled.
4. Losing branch has acquired resources. Resources are disposed during teardown.
5. Race with 3+ branches.
6. Nested race.

### Timeout tests

1. Body completes before timeout. Timer cancelled. Body result delivered.
2. Timeout fires before body completes. Body cancelled. Timeout error propagated.
3. Timeout with tryCatch: timeout error caught by recovery branch.
4. Timeout with RAII: body acquires resource, timeout fires, resource disposed.

## Deliverables

1. Bracket handler DAG (track resources in state, dispose on exit via `StateUpdate::Updated`)
2. Bracket Handle frame logic (state-based resource tracking, dispose on scope exit)
3. `race()` TypeScript function
4. `withTimeout()` TypeScript function
5. Timer action (Invoke that external driver resolves after duration)
6. Tests per above
7. Demo: add timeout and error handling to an existing demo workflow
