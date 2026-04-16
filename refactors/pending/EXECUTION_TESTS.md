# Execution Tests for All Combinators

**Status:** Stub — expand after UNION_DISPATCH_AST_NODES lands.

## Problem

Current tests are mostly AST-shape assertions (patterns.test.ts) and type-level checks (types.test.ts). They verify that combinators produce the right AST nodes, but don't verify that those AST nodes execute correctly through the Rust engine. A combinator could produce a structurally correct AST that the engine misinterprets.

## Goal

Every public method and function in the barnum SDK should have at least one test that builds a pipeline, executes it end-to-end through the Rust engine, and asserts on the output value. This catches:

- Serialization mismatches between TS AST construction and Rust AST deserialization
- Engine execution bugs for specific node combinations
- Regressions when builtins or branch logic changes

## Scope

TBD — enumerate all public combinators and identify coverage gaps.
