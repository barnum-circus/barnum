# Phase 4: Loop Migration

## Goal

Migrate the existing LoopAction from a dedicated frame kind to Handle/Perform sugar. After this phase, LoopAction is removed from both the tree AST and the Rust scheduler. Loop behavior is unchanged — existing tests provide regression coverage.

## Prerequisites

Phase 1 (Effect Substrate) complete. Phase 3's teardown (discarding continuations) is needed for both the Break and Continue paths — both discard the current continuation.

## Two effects, not one

Loop uses two separate effects with two nested Handles. There is no single "LoopControl" effect type with a Continue/Break payload. The routing is structural (which Handle catches it), not payload-based.

- `recur` = `Perform(recurEffect)` → caught by inner Handle → `RestartBody` → restart the body
- `done` = `Perform(doneEffect)` → bubbles past inner Handle → caught by outer Handle → `Discard` → exit the loop

Both paths discard the current continuation. They differ in what the Handle frame does next: re-enter the body (recur) or exit (done).

## How loop compiles

```ts
// User writes:
loop((recur, done) =>
  pipe(body, branch({ HasErrors: pipe(fix, recur), Clean: done }))
)

// Builder gensyms two EffectIds: recurEffect, doneEffect
// recur = Perform(recurEffect)
// done = Perform(doneEffect)

// Compiles to:
Handle(doneEffect, tag("Discard"),
  Handle(recurEffect, tag("RestartBody"),
    body
  )
)
```

Each handler DAG is trivial — just tag the value with the continuation operation. No branching.

### Nested loops: labeled breaks for free

Each loop invocation mints its own pair of EffectIds. Nested loops have distinct Handles:

```ts
loop((recurOuter, doneOuter) =>
  loop((recurInner, doneInner) =>
    pipe(body, branch({
      ContinueInner: recurInner,
      BreakInner: doneInner,
      BreakBoth: doneOuter,       // breaks out of BOTH loops
    }))
  )
)
```

`doneOuter` bubbles past both inner Handles (wrong EffectIds) and is caught by the outer done Handle.

### HOAS is required

There are no standalone `recur()`/`done()` combinators. The HOAS callback is the only way to get the tokens. If you need to pass them to a utility function, pass them as parameters.

## Migration strategy

### Step 1: Compile loop to Handle/Perform

Change `loop()` to produce two nested Handles instead of LoopAction. The HOAS callback replaces the current standalone `recur()`/`done()`.

### Step 2: Verify test parity

All existing loop tests must pass with the new compilation. The outputs must be identical.

### Step 3: Remove LoopAction

Remove `LoopAction` from the tree AST union. Remove `FrameKind::Loop` from the Rust scheduler. Remove the flattener's Loop handling. Remove standalone `recur()`/`done()`.

### Step 4: Verify demos

Run all demos. They should work unchanged because the surface API (`loop`) is the same.

## Test strategy

### Regression tests

All existing loop tests must pass after migration:
- Simple loop with Break
- Loop with Continue (multiple iterations)
- Nested loops
- Loop inside ForEach
- Loop inside Parallel
- Loop with branch dispatching Continue/Break

### New tests

1. **recur/done outside loop**: Verify `UnhandledEffect` error.
2. **Labeled break**: Inner loop breaks out of outer loop via `doneOuter`.
3. **Loop with declare**: Variables from an outer declare are accessible inside the loop body, including across Continue re-entries.
4. **Loop with tryCatch**: Error inside loop body caught by tryCatch inside the loop. Loop continues.
5. **tryCatch around loop**: Error inside loop body propagates past the loop's Handles to the tryCatch Handle.

## Deliverables

1. `loop()` rewritten to produce two nested Handles (HOAS callback)
2. `recur` / `done` tokens provided via callback, not standalone combinators
3. Migration: test parity verified against existing loop tests
4. LoopAction and FrameKind::Loop removed from tree AST and Rust scheduler
5. All existing tests pass
