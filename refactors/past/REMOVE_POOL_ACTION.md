# Remove Pool Action Kind from Rust

**Parent:** TS_CONFIG.md
**Depends on:** POOL_TO_BASH (all configs converted, no Pool references in config files)

## Motivation

After POOL_TO_BASH lands, no config file uses `kind: "Pool"`. Pool exists only in Rust types and test helpers. This refactor removes it, leaving Command (to be renamed Bash) and TypeScript as the only action kinds.

## Config types

**File:** `crates/barnum_config/src/config.rs`

Remove `PoolActionFile` and the `Pool` variant from `ActionFile`:

```rust
// Before
pub enum ActionFile {
    Pool { params: PoolActionFile },
    Command { params: CommandActionFile },
}

// After
pub enum ActionFile {
    Command { params: CommandActionFile },
}
```

Delete `PoolActionFile` struct entirely. Delete `MaybeLinked<String>` if Pool was its only consumer (check `instructions` field usage).

## Resolved types

**File:** `crates/barnum_config/src/resolved.rs`

Remove `PoolAction` and the `Pool` variant from `ActionKind`:

```rust
// Before
pub enum ActionKind {
    Pool { params: PoolAction },
    Command { params: CommandAction },
}

// After
pub enum ActionKind {
    Command { params: CommandAction },
}
```

Delete `PoolAction` struct. Remove Pool-related resolution logic from `ConfigFile::resolve()`.

## Runner

**File:** `crates/barnum_config/src/runner/action.rs`

Delete the `PoolAction` struct (the runtime `Action` impl that spawns troupe). Delete `submit.rs` if it exists. Remove the `Pool` match arm from `dispatch_task` in `runner/mod.rs`.

**File:** `crates/barnum_config/src/runner/mod.rs`

Remove `Invoker<TroupeCli>` from `Engine` and `RunnerConfig`. Remove `has_pool_actions()` from `Config`.

## Docs generation

**File:** `crates/barnum_config/src/docs.rs`

`generate_step_docs` (the per-task docs generator used by Pool dispatch) is deleted — it moved to JS. `generate_full_docs` (used by `barnum config docs`) stays but needs updating: it currently reads `ActionKind::Pool { instructions }` to extract step instructions. After Pool removal, it reads from `CommandAction` or the opaque action JSON. If `barnum config docs` can't extract instructions from Command actions (they're bash scripts, not structured instructions), it may need to be simplified or removed.

## Tests

14 test files reference Pool actions. Each test helper that creates Pool actions needs to create Command actions instead. The test configs in `common/mod.rs` change from:

```rust
ActionFile::Pool { params: PoolActionFile { instructions: MaybeLinked::Inline("...".into()), .. } }
```

to:

```rust
ActionFile::Command { params: CommandActionFile { script: "echo '[]'".into() } }
```

Most tests don't care about the action kind — they test the state machine (retries, timeouts, transitions, concurrency). Replacing Pool with a Command that echoes `[]` (or the expected follow-up tasks) is sufficient.

## Generated schemas

After removing Pool from the Rust types, regenerate:
- `libs/barnum/barnum-config-schema.json`
- `libs/barnum/barnum-config-schema.zod.ts`
- `libs/barnum/barnum-resolved-schema.zod.ts`

Pool disappears from the generated schemas.

## What this does NOT do

- Does not add the TypeScript action kind (separate refactor)
- Does not rename Command to Bash (can be a follow-up or part of the TypeScript action kind refactor)
- Does not change `MaybeLinked` — if it's still used by other fields, it stays
