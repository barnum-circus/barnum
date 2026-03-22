# TypeScript Runtime

**Status:** Stub — deferred

## Motivation

`TYPESCRIPT_CONFIG.md` covers generating types and exporting them from the npm package. This document covers everything about executing TypeScript at runtime: the `barnum run --ts` CLI flag, discovering a TS runtime (node, tsx, bun), step builder helpers, and eventually programmatic `barnum.run()` from TypeScript.

## Scope (rough)

### `barnum run --ts <file>`

- New CLI flag, mutually exclusive with `--config`.
- Executes the TS file, extracts its default export, serializes to JSON, feeds it into the existing config pipeline.
- Requires a TS-capable runtime. Discovery order: env var, project-local tsx, global tsx, bun, node 22.6+ with `--experimental-strip-types`.
- Optional `--ts-runner` flag to override auto-detection.

### Programmatic `barnum.run()`

- `@barnum/barnum` exports a `run()` function that accepts a `ConfigFile` and options.
- Spawns the Rust binary as a subprocess with the config piped in.
- Returns a promise that resolves when the workflow completes.

## Dependencies

- `TYPESCRIPT_CONFIG.md` — TypeScript types and npm package exports must land first.

## Status

Not yet designed. This document captures the scope for future work.
