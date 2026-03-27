# Deferred Features

Features removed from the initial implementation to keep the surface area minimal. To be added incrementally as needed.

## Builtin Handler Kind

Rust-native data transformations executed without FFI. Conceptually a variant of `HandlerKind` (not a separate `Action` variant — it's a type of `Call`).

Operations:
- **Tag**: Wraps input as `{ kind, value: input }`. Enables `recur()` (Tag "Continue") and `done()` (Tag "Break") for loop signals.
- **Identity**: Passes input through unchanged.
- **Merge**: Merges an array of objects into a single object.
- **Flatten**: Flattens a nested array one level.
- **ExtractField**: Extracts a single field from an object.

Without Builtin, loop signals and structural transforms must be implemented in handler code (TypeScript).

## Handler Validator Ergonomics

Reconsider the `createHandler` validator design:

- **Optional `stepValueValidator`**: When omitted, the value type could default to `never` or `{}`, signaling "this handler doesn't consume pipeline input." Currently required.
- **`stepConfigValidator` as a type parameter instead of a runtime validator**: Instead of `stepConfigValidator?: z.ZodType<TStepConfig>`, allow passing `TStepConfig` as a generic type parameter directly (e.g., `createHandler<{ timeout: number }>({...})`). The runtime validator is needed for serialization, but the type parameter approach is more ergonomic for handlers where config shape is known statically.
- General question: should validators be the only way to specify types, or should explicit type parameters remain an option?

## Context

Read-only environment (`context: Value`) on `Config`, passed to all handlers. Carries API keys, workflow IDs, tenant config, etc.

Alternative: user-land Reader Monad pattern using `All` + `Identity` + `Merge` (see WORKFLOW_ALGEBRA.md). This incurs O(N) cloning cost for parallel branches, which the host-level context avoids.
