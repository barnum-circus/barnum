# Phase 4: Loop → Handle/Perform and Early Return

## Goal

Remove `LoopAction` from both the tree AST and Rust engine. `loop` becomes sugar over Handle/Perform/Branch — the same effect substrate used by tryCatch, race, and withTimeout. Add `scope` as a user-facing early-return primitive.

## Design

### Shared compilation pattern

Both `scope` and `loop` compile to the same structure:

```
Chain(
  Tag("Continue"),                          // wrap input as LoopResult::Continue
  Handle(effectId, RestartBodyHandler,
    Branch({
      Continue: <user body>,                // auto-unwraps value
      Break: identity(),                    // auto-unwraps, exits Handle
    })
  )
)
```

The handler receives `{ payload, state }` from the engine, extracts payload, and tags as `RestartBody`:

```ts
const RESTART_BODY_HANDLER: Action = Chain(ExtractField("payload"), Tag("RestartBody"));
```

Effects (recur/done/jump) are `Chain(Tag(<variant>), Perform(effectId))`. They tag the value as a LoopResult variant, then Perform sends it to the handler. The handler RestartBody's it back to the Branch, which dispatches Continue (rerun body) or Break (exit via identity).

### `loop` API change

```ts
// Before:
loop(pipe(body, branch({ HasErrors: pipe(fix, recur<any, void>()), Clean: done<any, void>() })))

// After:
loop((recur, done) =>
  pipe(body, branch({ HasErrors: pipe(fix, recur), Clean: done }))
)
```

`recur` and `done` are `TypedAction` values (not functions), consistent with `throwError` in tryCatch. They capture the effectId from the enclosing loop.

```ts
loop<TIn, TBreak, TRefs extends string = never>(
  bodyFn: (
    recur: TypedAction<TIn, never>,
    done: TypedAction<TBreak, never>,
  ) => Pipeable<TIn, never, TRefs>,
): TypedAction<TIn, TBreak, TRefs>
```

### `scope` — early return

```ts
scope<TIn, TJump, TOut, TRefs extends string = never>(
  bodyFn: (jump: TypedAction<TJump, never>) => Pipeable<TIn, TOut, TRefs>,
): TypedAction<TIn, TJump | TOut, TRefs>
```

`jump` is `Chain(Tag("Break"), Perform(effectId))`. If the body completes normally → output is TOut. If jump fires → output is TJump. The union `TJump | TOut` captures both paths.

### Why recur and done are both Performs

If `done` were normal completion, it would only exit the innermost loop. Making `done` a Perform means it bubbles through Handle frames until it finds its matching effectId — enabling labeled breaks across nested scopes for free.

## Tasks

### 1. Change `loop()` to callback form with Handle/Perform compilation

**File:** `libs/barnum/src/ast.ts`

- Change `loop()` signature to take `(recur, done) => body` callback
- Compile to `Chain(Tag("Continue"), Handle(effectId, RestartBodyHandler, Branch({Continue: body, Break: identity()})))`
- `recur` = `Chain(Tag("Continue"), Perform(effectId))`
- `done` = `Chain(Tag("Break"), Perform(effectId))`
- Remove `LoopAction` interface and `LoopAction` from `Action` union
- Keep `LoopResult`, `LoopResultDef`, `ExtractBreakValue` types (still used by engine result shape, and by `LoopResult` as a user-facing type)

### 2. Remove standalone `recur()` and `done()` from builtins

**File:** `libs/barnum/src/builtins.ts`

Remove the `recur()` and `done()` functions. They're replaced by callback parameters in loop().

### 3. Add `scope()` for early return

**File:** `libs/barnum/src/ast.ts` (or new `scope.ts`)

Same compilation as loop but exposes a single `jump` instead of recur/done. `jump` = `Chain(Tag("Break"), Perform(effectId))`.

### 4. Update TypeScript tests

- **patterns.test.ts**: Update loop AST shape assertions (now Handle/Branch instead of `{ kind: "Loop" }`)
- **types.test.ts**: Update loop type tests to callback form; add scope type tests
- **steps.test.ts**: Update all loop usages to callback form
- **round-trip.test.ts**: Update loop round-trip test (now produces Handle/Branch JSON)

### 5. Remove LoopAction from Rust AST

**File:** `crates/barnum_ast/src/lib.rs`

- Remove `Action::Loop(LoopAction)` variant
- Remove `LoopAction` struct

**File:** `crates/barnum_ast/src/flat.rs`

- Remove `FlatAction::Loop` variant
- Remove flattening code for `Action::Loop`
- Update tests that use `FlatAction::Loop`

### 6. Remove FrameKind::Loop from Rust engine

**File:** `crates/barnum_engine/src/frame.rs`

- Remove `ParentRef::Loop` variant
- Remove `FrameKind::Loop` variant

**File:** `crates/barnum_engine/src/lib.rs`

- Remove `CompleteError::InvalidLoopResult`
- Remove `ParentRef::Loop` handling in `deliver()`
- Remove `FlatAction::Loop` handling in `advance()`
- Update tests (loop tests now exercise Handle/Branch path instead)

### 7. Update demos

All demos mechanically change from `recur()` / `done()` imports to callback form. No behavioral changes.

### 8. Add step D + early return to retry-on-error demo

Add a `stepD` handler and use `scope(jump => ...)` to wrap the tryCatch loop. stepD.unwrapOr(jump) exits the entire workflow on catastrophic failure instead of retrying.

## Files changed

| File | Change |
|------|--------|
| `libs/barnum/src/ast.ts` | Remove LoopAction, change loop() to callback+Handle/Perform, add scope() |
| `libs/barnum/src/builtins.ts` | Remove recur(), done() |
| `libs/barnum/tests/patterns.test.ts` | Loop AST shape → Handle/Branch |
| `libs/barnum/tests/types.test.ts` | Callback loop, scope types |
| `libs/barnum/tests/steps.test.ts` | Callback loop syntax |
| `libs/barnum/tests/round-trip.test.ts` | Loop → Handle/Branch JSON |
| `crates/barnum_ast/src/lib.rs` | Remove Action::Loop, LoopAction |
| `crates/barnum_ast/src/flat.rs` | Remove FlatAction::Loop |
| `crates/barnum_engine/src/frame.rs` | Remove ParentRef::Loop, FrameKind::Loop |
| `crates/barnum_engine/src/lib.rs` | Remove Loop handling, InvalidLoopResult |
| `demos/*/run.ts` | Callback loop syntax |
| `demos/retry-on-error/` | Add stepD + scope early return |
