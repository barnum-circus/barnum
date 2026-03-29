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
declare({ wt: pipe(deriveBranch, createWorktree) }, ({ wt }) => body)
// Where createWorktree has dispose metadata.

// Compiles to nested handlers:
Chain(
  pipe(deriveBranch, createWorktree),
  Handle("Bracket", {
    // Bracket handler stores the resource and manages disposal
    Handle("ReadVar", {
      body   // uses Perform(ReadVar) to access the worktree
    })
  })
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

// Compiles to:
Handle(
  { "FirstResult": pipe(pick("payload"), identity()) },
  Parallel(
    Chain(actionA, Perform("FirstResult")),
    Chain(actionB, Perform("FirstResult")),
  )
)
```

The handler receives the first completion and exits the Handle frame. The handler does NOT resume (no Resume in the DAG). On Handle exit, the un-completed Parallel branch is torn down via the standard continuation cleanup.

### Cancellation semantics

When the Handle exits with a live Parallel frame below it, teardown must:
1. Cancel pending external tasks in the losing branch.
2. Run Bracket dispose for any resources the losing branch acquired.
3. Remove all frames from the slab.

This uses the same `teardown_continuation` from Phase 3. Race doesn't add new teardown logic — it exercises existing teardown under Parallel.

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

// Compiles to:
Handle(
  { "Timeout": pipe(pick("payload"), throwTimeoutError) },
  race(body, timer(duration))
)
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

1. `EffectType::Bracket` and `EffectType::FirstResult` variants
2. Bracket Handle frame logic (track resources, dispose on exit)
3. `race()` TypeScript function
4. `withTimeout()` TypeScript function
5. Timer action (Invoke that external driver resolves after duration)
6. Tests per above
7. Demo: add timeout and error handling to an existing demo workflow
