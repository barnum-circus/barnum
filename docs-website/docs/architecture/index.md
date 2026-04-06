# Architecture

How Barnum works under the hood.

## Overview

```
TypeScript DSL (libs/barnum)
  → Serializable AST (JSON)
    → Rust engine (crates/barnum_engine)
      → Event loop + scheduler (crates/barnum_event_loop)
        → Handler subprocess execution (crates/barnum_typescript_handler)
```

The TypeScript library defines the workflow. `workflowBuilder().run()` serializes the AST to JSON and spawns the Rust binary, which flattens the AST into a `FlatConfig`, manages frames and task dispatch, and executes each handler as an isolated subprocess. Input and output schemas (defined via Zod) are compiled into JSON Schema validators at init and enforced at every handler boundary.

## Sections

- [TypeScript AST](./typescript-ast.md) — how the TypeScript DSL produces a serializable AST
- [Compiler](./compiler.md) — how the Rust compiler flattens the AST into an executable form
- [Algebraic effect handlers](./algebraic-effect-handlers.md) — how `loop`, `tryCatch`, and `earlyReturn` work under the hood
