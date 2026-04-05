# Eliminate Discard HandlerOutput

Pre-refactor: rewrite tryCatch and race to use restart+Branch instead of Discard. After this lands, nothing produces `Discard`, and the variant (plus supporting code) can be deleted.

## TypeScript changes

### tryCatch (`try-catch.ts`)

**Before:**

```ts
// throwError = Perform(effectId)
// handler = Chain(ExtractField("payload"), Chain(recovery, Tag("Discard")))
Handle(effectId, body, handler)
```

**After:**

```ts
// throwError = Chain(Tag("Break"), Perform(effectId))
// Uses buildLoopAction: Chain(Tag("Continue"), Handle(effectId, Branch({ Continue: body, Break: recovery }), RestartBodyHandler))
buildLoopAction(effectId, body)  // with Break arm = recovery instead of identity
```

`throwError` changes from bare `Perform(effectId)` to `Chain(Tag("Break"), Perform(effectId))`. The error is tagged Break before the Perform. Handler extracts payload (the tagged value), tags RestartBody, engine restarts. Branch sees `{ kind: "Break", value: error }`, `unwrapBranchCases` extracts `value`, recovery receives the error. Recovery completes, body completes, Handle exits.

Need a generalized version of `buildLoopAction` that accepts a custom Break arm (tryCatch passes `recovery`, loop passes `IDENTITY`).

### race (`race.ts`)

**Before:**

```ts
// Each branch: Chain(action, Perform(effectId))
// Handler: Chain(ExtractField("payload"), Tag("Discard"))
Handle(effectId, All(branches...), handler)
```

**After:**

```ts
// Each branch: Chain(action, Chain(Tag("Break"), Perform(effectId)))
// Uses buildLoopAction pattern with Break arm = identity
Chain(Tag("Continue"), Handle(effectId, Branch({ Continue: All(branches...), Break: identity() }), RestartBodyHandler))
```

Each branch's Perform becomes `Chain(Tag("Break"), Perform(effectId))`. Winner tags Break, performs. Handler restarts. Branch takes Break, identity, Handle exits.

### withTimeout (`race.ts`)

Same pattern as race. Each branch (body and sleep) wraps its result (Ok/Err tag) then `Chain(Tag("Break"), Perform(effectId))`.

### Shared changes

Export `buildLoopAction` or generalize it. Currently it hardcodes `Break: IDENTITY`. tryCatch needs `Break: recovery`. Options:

1. Add a `breakArm` parameter: `buildRestartBranchAction(effectId, body, breakArm)`
2. tryCatch constructs the AST directly (duplicating the pattern)

Option 1 is cleaner. Rename `buildLoopAction` to `buildRestartBranchAction(effectId, continueArm, breakArm)`.

The shared constants `TAG_CONTINUE`, `TAG_BREAK`, `RESTART_BODY_HANDLER` are already in `ast.ts`. tryCatch and race need access to them. Either:

- Export them from `ast.ts`
- Move them to a shared module
- Duplicate them in `try-catch.ts` and `race.ts`

Exporting from `ast.ts` is simplest. `RESTART_BODY_HANDLER`, `TAG_CONTINUE`, `TAG_BREAK`, and `unwrapBranchCases` are already used by `buildLoopAction` in `ast.ts`. Export `buildRestartBranchAction` and have tryCatch/race call it.

## Rust engine changes

### Delete `HandlerOutput::Discard` (`lib.rs:117-121`)

Remove the variant. `handle_handler_completion` match becomes two arms: `Resume` and `RestartBody`.

### Delete `discard_continuation` (`lib.rs:568-600`)

The method is only called from the `Discard` match arm. Delete it.

### Delete `BuiltinKind::TagDiscard` (`barnum_ast/src/lib.rs:254-255`)

No combinator produces `Tag("Discard")` anymore. Delete the variant and its execution case in `barnum_builtins`.

## Test changes

### Engine unit tests (`lib.rs`)

Tests that use `always_discard_handler` or `{"kind": "Discard", ...}` JSON:

| Test | Change |
|------|--------|
| `handle_discard_skips_rest_of_chain` | Rewrite: construct restart+Branch AST manually. Handler always restarts, Break arm = identity. Verify same outcome (body chain skipped, Handle exits). |
| `discard_cleans_up_frames_and_tasks` | Same rewrite. Verify frames cleaned up. |
| `restart_body_multiple_then_discard` | Final iteration: handler returns `RestartBody` with a Break-tagged value instead of `Discard`. Body Branch takes Break arm, exits. |
| `stash_dropped_after_discard` | Rewrite with restart+Branch. Stash behavior is the same (body still gets torn down on restart). |
| `throw_proceeds_while_resume_handler_in_flight` | Uses `always_discard_handler` for outer Handle. Rewrite outer Handle as restart+Branch. |

### Completion snapshot fixtures

`handle_discard.json` — rewrite with restart+Branch AST. Update snapshot.

### TypeScript integration tests

Run `pnpm test` to verify tryCatch/race/withTimeout demos still work.

## Sequencing

1. Generalize `buildLoopAction` → `buildRestartBranchAction(effectId, continueArm, breakArm)` in `ast.ts`. Export it.
2. Rewrite `tryCatch` in `try-catch.ts`.
3. Rewrite `race` and `withTimeout` in `race.ts`.
4. Run `pnpm run typecheck`.
5. Delete `HandlerOutput::Discard`, `discard_continuation`, `BuiltinKind::TagDiscard`.
6. Rewrite engine unit tests.
7. Rewrite `handle_discard.json` completion fixture.
8. Run `cargo test`, `pnpm test`.
