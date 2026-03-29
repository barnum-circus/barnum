# Phase 4: Loop Migration (LoopControl Effect)

## Goal

Migrate the existing LoopAction from a dedicated frame kind to Handle/Perform sugar. After this phase, LoopAction is removed from both the tree AST and the Rust scheduler. Loop behavior is unchanged — existing tests provide regression coverage.

## Prerequisites

Phase 1 (Effect Substrate) complete. Phases 2 and 3 are not strict prerequisites, but Phase 3's teardown work (discarding continuations) is needed for the Break path.

## The effect

```
Effect: LoopControl
Payload: { kind: "Continue", value: Value } | { kind: "Break", value: Value }
Handler behavior:
  Continue: Discard the old continuation. Re-enter the body with the new value.
  Break: Discard the continuation. Exit the Handle frame with the value.
```

Both paths are zero-shot: the continuation from the Perform site is always discarded. Continue starts a fresh body execution. Break exits the scope. No multi-shot continuations.

## How loop compiles

```ts
// User writes:
loop(body)

// Compiles to:
Handle(
  { "LoopControl": loopHandler },
  body   // contains Perform(LoopControl) via recur() and done()
)
```

The handler DAG for LoopControl dispatches on Continue vs Break:

```ts
// Handler receives: { payload: { kind: "Continue"|"Break", value }, cont_id }
pipe(
  pick("payload"),
  branch({
    Continue: pipe(
      extractField("value"),
      // Re-enter the body. This is a jump back to the Handle's body ActionId.
      // The cont_id is NOT used — the old continuation is discarded on Handle exit.
      ReEnterBody(),
    ),
    Break: pipe(
      extractField("value"),
      // Exit the Handle frame with this value.
      // Again, cont_id is not used.
      ExitHandle(),
    ),
  }),
)
```

`ReEnterBody` and `ExitHandle` are new builtins (or effect-handler-specific actions) that the Handle frame understands. Alternatively, the Handle frame can inspect the handler DAG's output and decide based on a tagged union.

### Alternative: Handle frame has built-in Continue/Break semantics

Instead of the handler DAG doing the dispatch, the Handle frame for LoopControl could have hardcoded behavior:

```rust
// In the Handle frame's effect dispatch:
match effect {
    EffectType::LoopControl => {
        match payload.get("kind") {
            "Continue" => {
                self.teardown_continuation(cont_root);
                self.advance_body(payload.get("value"));
            }
            "Break" => {
                self.teardown_continuation(cont_root);
                self.deliver_to_parent(payload.get("value"));
            }
        }
    }
}
```

This is simpler. The handler DAG is unnecessary for LoopControl because the behavior is fully determined by the payload. The Handle frame does the dispatch internally.

Recommendation: if we're using an enum for effect types, the Handle frame can have per-effect-type logic. LoopControl's logic is hardcoded in the Handle frame's dispatch. No handler DAG needed.

If we later move to string-based effects, the handler DAG approach becomes necessary because the Handle frame can't know what "my_app:retry" means. But for framework-level effects (LoopControl, ReadVar, Throw), hardcoded logic is simpler.

## recur() and done() rewrite

Currently:
- `recur()` compiles to a builtin that produces `{ kind: "Continue", value }` (a tagged union that Loop's frame kind understands).
- `done()` compiles to a builtin that produces `{ kind: "Break", value }`.

After migration:
- `recur()` compiles to `pipe(tag("Continue"), Perform("LoopControl"))`.
- `done()` compiles to `pipe(tag("Break"), Perform("LoopControl"))`.

The surface API doesn't change. `recur<TIn, TOut>()` and `done<TIn, TOut>()` still have the same type signatures. Only the AST they produce changes.

## Migration strategy

### Step 1: Implement LoopControl effect in Handle frame

Add `EffectType::LoopControl` to the enum. Add the Continue/Break dispatch logic to the Handle frame's effect handler. This runs alongside the existing LoopAction frame kind.

### Step 2: Add a loop-via-Handle compilation path

In TypeScript, add a flag or separate function (`loopV2`?) that compiles to Handle/Perform instead of LoopAction. Run all existing loop tests against both paths.

### Step 3: Verify test parity

Every existing loop test must pass with both the old LoopAction path and the new Handle/Perform path. The outputs must be identical.

### Step 4: Switch default compilation

Change `loop()` to compile to Handle/Perform. The old LoopAction path is dead code.

### Step 5: Remove LoopAction

Remove `LoopAction` from the tree AST union. Remove the Loop frame kind from the Rust scheduler. Remove the flattener's Loop handling. The only loop mechanism is Handle(LoopControl).

### Step 6: Verify demos

Run all demos. They should work unchanged because the surface API (`loop`, `recur`, `done`) is the same.

## HOAS opportunity

Currently, `recur()` and `done()` are standalone combinators. With HOAS, loop could provide them as callback parameters:

```ts
// Current:
loop(
  pipe(body, branch({ HasErrors: pipe(fix, recur()), Clean: done() }))
)

// With HOAS:
loop((recur, done) =>
  pipe(body, branch({ HasErrors: pipe(fix, recur()), Clean: done() }))
)
```

The HOAS version ensures recur/done can only be used within the loop body (TypeScript scoping enforces this). The non-HOAS version allows recur/done to be used outside a loop, which is a runtime error ("unhandled LoopControl effect").

Recommendation: add the HOAS form as the primary API. Keep the standalone `recur()`/`done()` for backward compatibility and for use inside `registerSteps` where the loop callback isn't available.

## Test strategy

### Regression tests

All existing loop tests must pass after migration. These include:
- Simple loop with Break
- Loop with Continue (multiple iterations)
- Nested loops
- Loop inside ForEach
- Loop inside Parallel
- Loop with branch dispatching Continue/Break

### New tests

1. **recur/done outside loop**: Verify `UnhandledEffect(LoopControl)` error.
2. **Loop with declare**: Variables from an outer declare are accessible inside the loop body, including across Continue re-entries.
3. **Loop with tryCatch**: Error inside loop body caught by tryCatch inside the loop. Loop continues.
4. **tryCatch around loop**: Error inside loop body propagates past the loop's Handle to the tryCatch Handle.

## Deliverables

1. `EffectType::LoopControl` variant
2. Handle frame LoopControl dispatch (Continue → re-enter body, Break → exit)
3. `recur()` / `done()` rewritten to Perform(LoopControl)
4. `loop()` rewritten to produce Handle(LoopControl, body)
5. HOAS form: `loop((recur, done) => body)`
6. Migration: both paths run in parallel, test parity verified
7. LoopAction removed from tree AST and Rust scheduler
8. All existing tests pass
