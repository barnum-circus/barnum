# Incremental compilation / caching

Userland follow-on to `PRECOMPILATION.md`. Builds on `compile()` + `runCompiled`;
no engine changes. Out of scope for the initial postfix pass.

## Goal

Avoid re-walking the TypeScript AST when the workflow source hasn't changed between runs.

## Approach

Caching is expressible entirely in userland with the primitives from `PRECOMPILATION.md`:

1. Hash the TypeScript source + handler files.
2. On a hit: read the cached config JSON and run it via `CompiledWorkflow.fromJSON(cachedJson).run()`.
3. On a miss: `pipeline.compile()`, write `compiled.configJson` to the cache, then `.run()`.

Analogous to `tsc --incremental` — the cached artifact is a transparent optimization. Because both paths feed the same serialized config into the same spawn path, execution is identical downstream.

## Notes

- The framework owns no file I/O; the hashing, cache directory, and read/write are the user's (or a thin helper's) responsibility, consistent with `PRECOMPILATION.md`.
- Deterministic compilation (`flatten`) is what makes the cache sound: same source → same JSON.
