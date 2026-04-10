# Backlog

## Builtins

- **Add circular import lint rule** — oxlint rule to disallow circular imports in `libs/barnum/src/`. Blocked: oxlint native binding broken (pnpm optional dep issue). Fix with `pnpm install` and configure `import/no-cycle` in `.oxlintrc.json`.

## Docs written (pending review)

- Pre-compilation / serialization — PRECOMPILATION.md
- Loop with closure providing scoped recur/done — LOOP_WITH_CLOSURE.md
- Future combinators (getIndex, option-returning getField, unwrap, pick, withResource redesign) — FUTURE_COMBINATORS.md

## Fixes

- **Fix poll status** — The simple-workflow poll-status demo is broken. Investigate and fix (or document how to fix).
