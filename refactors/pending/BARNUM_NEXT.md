# Barnum Next: Structural / Architectural

Sections 1–4 (curried withTimeout, withRetries, allObject, array ops) have been consolidated into `API_SURFACE_AUDIT.md`.

This doc retains only the structural/architectural concerns.

---

## 1. Colocate tests

Tests are in `libs/barnum/tests/` instead of next to source files.

**Current:** `tests/patterns.test.ts` is a grab-bag of AST structure tests for pipe, all, branch, loop, bind, forEach, race, tryCatch — all in one file. Finding tests for a given combinator requires searching.

**Proposed:** Split into colocated files: `src/builtins.test.ts`, `src/pipe.test.ts`, `src/bind.test.ts`, `src/race.test.ts`, etc. Each test file tests exactly the module it sits next to.

The test helper `handlers.ts` stays in `tests/` or becomes `src/__test__/handlers.ts`.

Same principle on Rust side: tests for builtin execution should live next to the builtin implementation.

## 2. Reduce builtin definition boilerplate

Adding a new JS builtin requires touching five files: `ast.ts` (BuiltinKind type), `builtins.ts` (function), `index.ts` (re-export), plus Rust AST and Rust implementation.

**TS-side fix options:**

1. **`export *` from builtins.ts** — The barrel `index.ts` already re-exports from `builtins.ts`. The explicit list exists because `Option` and `Result` need declaration merging. Use `export * from "./builtins.js"` and only keep the declaration merge.

2. **Derive BuiltinKind from constructors** — The `BuiltinKind` type in `ast.ts` is maintained separately from the functions that construct those nodes. If builtins.ts is the single source of truth, the type union can be derived or simply not exported (it's an internal wire format).

## 3. List vs Array naming

Keep "array." The TS ecosystem universally uses Array/ReadonlyArray. Fighting the language's naming creates confusion. If a standalone syntax emerges, "list" could be the surface syntax name that compiles to array operations underneath.
