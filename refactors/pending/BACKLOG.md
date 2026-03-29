# Backlog

## Renames

- **Rename `configBuilder` to `workflowBuilder`** — The builder constructs a workflow, not a config. Update `ast.ts` export, all demos, and all tests.
- **Rename `stepValueValidator` to `inputValidator`** — Clearer name. Update `handler.ts`, all handler definitions in demos, and tests.

## Builtins

- **Add `extractIndex` builtin** — Extract element from JSON array by index. Enables tuple-based `withResource` redesign. See FUTURE_COMBINATORS.md.
- **Add circular import lint rule** — ESLint rule to disallow circular imports in `libs/barnum/src/`.

## Docs written (pending review)

- Pre-compilation / serialization — PRECOMPILATION.md
- Loop with closure providing scoped recur/done — LOOP_WITH_CLOSURE.md
- Future combinators (extractIndex, option-returning extractField, unwrap, pick, withResource redesign) — FUTURE_COMBINATORS.md

## Fixes

- **Fix poll status** — The simple-workflow poll-status demo is broken. Investigate and fix (or document how to fix).
