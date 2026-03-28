# Backlog

## Rust crate extraction

- **Extract TypeScript handler execution into its own crate** — `execute_typescript` and subprocess logic out of `barnum_event_loop`.
- **Extract builtins into their own crate** — `execute_builtin` and `BuiltinError` out of `barnum_event_loop`. Keep the `HandlerKind` enum that knows about all handler types for now.
- **Remove noop execution mode** — Delete `ExecutionMode::Noop`, `Scheduler::new()`, `Default` impl. Rewrite event_loop tests to use builtin handlers (e.g. `BuiltinKind::Constant`) instead of noop TypeScript stubs. Rename `with_executor` → `new`.

## Docs to write

- **Handler trait + registration system** — All handlers implement a trait. The engine doesn't know handler details. Some sort of registration system. Decouples engine from specific handler implementations.
- **Pre-compilation / serialization** — Speculate about storing compiled workflow state for resumption and performance. Consider contextual effects for reading input.
- **Loop with closure providing scoped recur/done** — `loop(({ recur, done }) => body)` where recur/done are properly typed objects scoped to the loop instance, not top-level exports. Closure called at construction time to build AST.

## Demos

- **Parallel refactor demo** — Iterate over items in pairs. For each: create random variable, create work tree, implement refactor, commit, make PR, delete work tree. Handlers just need correct types.

## Fixes

- **Fix poll status** — The simple-workflow poll-status demo is broken. Investigate and fix (or document how to fix).
