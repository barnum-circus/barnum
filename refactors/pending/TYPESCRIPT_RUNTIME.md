# TypeScript Runtime API

**Status:** Stub — deferred

## Motivation

Beyond using TypeScript to define configs (`TYPESCRIPT_CONFIG.md`), the next step is calling `barnum.run()` programmatically from a TypeScript script. The script would construct a config, provide initial tasks, and start the engine directly without going through the CLI.

This is a separate concern from the config API and depends on it being in place first.

## Scope (rough)

- `@barnum/barnum` exports a `run()` function (or similar) that accepts a `ConfigFile` and options.
- The function spawns the Rust binary as a subprocess with the config piped in (similar to how `cli.js` works today).
- Alternatively, the Rust engine could expose a C FFI or WASM interface for direct in-process execution, but subprocess is simpler and avoids the FFI boundary.
- The function returns a promise that resolves when the workflow completes, or rejects on failure.
- State log path, pool config, and other `RunnerConfig` fields are passed as options.

## Dependencies

- `TYPESCRIPT_CONFIG.md` — TypeScript types and `--ts` support must land first.
- `PLUGGABLE_ACTION_KINDS.md` — if TypeScript handlers (not just configs) are part of the runtime API, the executor trait must exist.

## Status

Not yet designed. This document exists to capture the intent and track it as a future work item.
