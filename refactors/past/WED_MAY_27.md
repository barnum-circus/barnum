## Done
- [x] readonly
- [x] update pending refactors, remove final --- section and update accordingly
- [x] test helper for asserting input and output types in one go (CheckIO)
- [x] sort inputs lint (eslint/sort-imports with ignoreDeclarationSort)
- [x] do not duplicate helpers — already centralized in tests/type-utils.ts
- [x] clean up toAction in postfix methods — use typedAction({ kind: "Chain", first: this, rest: ... }) directly

## Summary of pending refactor updates

Moved to past (done):
- CALLBACK_DESTRUCTURING (withResource a002b1b1, fold 1102ff74)
- DEFINE_RECURSIVE_FUNCTIONS, FOLD_AND_SPLITS, TAKE_AND_SKIP, WORKFLOW_OUTPUT, TS_VS_RUST_TRANSFORMS (moved in earlier commit)

Ready to work on now (approved/unblocked):
- NEWTYPES — use Newtype<TName, TInner> type alias everywhere, branch for unwrapping
- THUNK_BUILTINS — not blocked, can proceed independently
- REJECT_UNDEFINED_IN_PIPELINE — approved, needs research on specific constraint
- PRECOMPILATION — postfix `pipeline.run()` API approved, can implement
- UNFOLD — approved but may just be postfix loop, needs resolution with POSTFIX_LOOP

Decisions made:
- CONCURRENCY_LIMIT — Option A out, chunks method in userland initially
- EVENT_BUS — build on resume handle, `maybeDequeue` only, no new primitives
- LAZY_ITERATORS — simplify to flatMap-based model
- SCHEMA_COMMENTS — Option A (Zod .describe() → JSON Schema)
- RAII — needs engine primitives, not userland-only
- EFFECTS_PHASE_6_DURABLE — serialize stack tree, not algebraic effects

Deferred/unclear:
- SECOND_CLASS_FUNCTIONS — not worth doing yet
- VOID_INPUTS — likely decided against
- EFFECTS_DEFERRED — unclear if needed
- RECURSIVE_SCHEMAS — may not matter since schemas go through files

## TODO
- [x] are you fucking crazy? You fucking implemented the "assertIO" thing but didnt' fucking go back and use it. FUCKING GO BACK AND FUCKING USE IT.
- [x] ast.ts remaining toAction uses in exported fns (forEach, branch, loop, etc.) — these are legitimate (erasing phantoms for Action-typed struct fields). May not need further cleanup.
- [x] type tests need improvement: assertions should test input and output types are what we expect, var refs have appropriate types, and negative cases are tested
- [x] all methods should accept var ref's — see VARREF_CALLBACK_OVERLOADS.md (deferred; methods stay as Pipeable, bindInput is user-facing opt-in)
- [x] replace `assertExact<CheckIO<...>>` with a single `assertIO<TAction, TIn, TOut>()` helper and migrate all tests