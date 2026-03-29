# Backlog

## Renames

- **Rename `configBuilder` to `workflowBuilder`** — The builder constructs a workflow, not a config. Update `ast.ts` export, all demos, and all tests.

## Docs to write

- **Handler trait + registration system** — All handlers implement a trait. The engine doesn't know handler details. Some sort of registration system. Decouples engine from specific handler implementations.
- **Pre-compilation / serialization** — Speculate about storing compiled workflow state for resumption and performance. Consider contextual effects for reading input.
- **Loop with closure providing scoped recur/done** — `loop(({ recur, done }) => body)` where recur/done are properly typed objects scoped to the loop instance, not top-level exports. Closure called at construction time to build AST.

## Fixes

- **Fix poll status** — The simple-workflow poll-status demo is broken. Investigate and fix (or document how to fix).
