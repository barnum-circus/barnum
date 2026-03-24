# Add TypeScript Action Dispatch (Rust)

**Status:** Done
**Parent:** TS_CONFIG.md
**Depends on:** UNIFY_STDIN_ENVELOPE, INLINE_RESOLVED_CONFIG
**Branch:** `add-typescript-dispatch` (rebased on master after `demo-executor-flags` landed)

## Summary

Added a `TypeScript` variant to `ActionKind`. From Rust's perspective, it's just a different shell command — the executor runs `run-handler.ts` with the handler path and export name as arguments. Also removed the vestigial `Action` trait and wired `--executor` / `--run-handler-path` as required CLI flags.

## What was done

1. Added `TypeScriptAction` struct and `TypeScript` variant to `ActionKind` in `config.rs`
2. Added `step_config` to `Envelope` and `ShellAction` in `action.rs`
3. Extracted shared dispatch body in `mod.rs` — both Bash and TypeScript arms produce `(script, step_config)`, fed into a single `ShellAction`
4. Added `executor` and `run_handler_path` to `RunnerConfig` and `Engine`
5. Made `--executor` and `--run-handler-path` required hidden CLI flags (injected by `cli.cjs` and `run.ts`)
6. Regenerated schemas
7. Removed vestigial `Action` trait — `ShellAction` now has inherent `start` method, `run_action`/`spawn_worker` take `ShellAction` directly

## Prerequisite landed independently on master

`demo-executor-flags` branch (merged to master first):
- Added `--run-handler-path` as hidden optional CLI arg (ignored on master)
- Updated `run.ts` to inject `--executor` and `--run-handler-path` when spawning barnum
- Updated `cli.cjs` to inject `--run-handler-path` alongside `--executor`
- Validates `--run-handler-path` exists on Rust side
- Created placeholder `libs/barnum/actions/run-handler.ts`

## What this does NOT do

- TypeScript handler interface (ADD_HANDLER_VALIDATION)
- run-handler.ts implementation (ADD_RUN_HANDLER)
- Path resolution (ADD_RUN_HANDLER — JS layer resolves before passing to Rust)
