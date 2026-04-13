# Architecture

Barnum is a TypeScript DSL that compiles to a serializable AST, which is executed by a Rust runtime. TypeScript handles authoring and type inference; Rust handles execution, schema validation, and concurrent subprocess management.

```
TypeScript DSL
  → Serializable AST (JSON)
    → Rust compiler (flatten tree → FlatConfig)
      → Event loop (advance / dispatch / complete)
        → Isolated handler subprocesses
```

## The pipeline

### 1. Authoring

Handlers and combinators are composed in TypeScript. Phantom types enforce that `pipe(a, b)` only compiles if `a`'s output type matches `b`'s input type. All type information exists purely for the compiler — it's erased before serialization.

### 2. AST serialization

`runPipeline()` calls `JSON.stringify()` on the composed workflow. Phantom fields and handler implementations are non-enumerable, so they're invisible to serialization. What remains is a clean JSON tree of nine action types: `Invoke`, `Chain`, `All`, `ForEach`, `Branch`, `ResumeHandle`, `ResumePerform`, `RestartHandle`, and `RestartPerform`.

### 3. Compilation

The Rust binary receives the JSON AST and flattens it into a `FlatConfig`: a linear array of 8-byte entries with index-based cross-references. No pointers, no heap allocation per entry. Identical handlers are interned — a handler used in 100 `forEach` iterations is stored once.

### 4. Execution

The event loop drives a pure state machine. `advance()` expands actions into frames (the runtime stack). `complete()` delivers results upward — Chain trampolines to the next step, All/ForEach collect results, RestartHandle re-advances the body. Effects (loop, tryCatch, earlyReturn) are implemented as algebraic effect handlers with deferred restart semantics.

### 5. Validation

Zod schemas on handlers are compiled to JSON Schema at definition time, embedded in the AST, and compiled into `jsonschema::Validator` instances at workflow init. Every handler invocation is validated twice: input before dispatch, output after completion.

## Sections

- [TypeScript AST](./typescript-ast.md) — how the DSL produces a serializable, type-safe AST
- [Postfix methods](./postfix-methods.md) — how `this` parameter constraints enable fluent chaining with type narrowing
- [Compiler and execution model](./compiler.md) — how the tree is flattened and executed
- [Algebraic effect handlers](./algebraic-effect-handlers.md) — how `loop`, `tryCatch`, and `earlyReturn` work
- [Validation](./validation.md) — how Zod serves as the single source of truth for types and runtime checks
