# Action Params Nesting

## Motivation

Action configs currently mix the discriminant (`kind`) with action-specific fields at the same level:

```json
{
  "kind": "Pool",
  "instructions": {"kind": "Inline", "value": "..."},
  "pool": "agents",
  "root": "/tmp/troupe",
  "timeout": 120
}
```

There's no separation between framework-controlled keys and action-specific parameters. This makes it impossible to add framework-level fields (e.g., a future `retry_policy` or `description`) without risking collisions with action-specific params. It also makes the structure harder to reason about generically — code that processes actions needs to know which keys belong to the framework and which belong to the action.

The fix: nest action-specific fields under a `params` key. The top level is owned by the framework and contains `kind` plus any future framework fields. Action-specific data lives in `params`.

```json
{
  "kind": "Pool",
  "params": {
    "instructions": {"kind": "Inline", "value": "..."},
    "pool": "agents",
    "root": "/tmp/troupe",
    "timeout": 120
  }
}
```

## Current State

### Config types (`crates/barnum_config/src/config.rs:132-182`)

```rust
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct PoolActionFile {
    pub instructions: crate::maybe_linked::MaybeLinked<Instructions>,
    #[serde(default)]
    pub pool: Option<String>,
    #[serde(default)]
    pub root: Option<PathBuf>,
    #[serde(default)]
    pub timeout: Option<u64>,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CommandActionFile {
    pub script: String,
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum ActionFile {
    Pool(PoolActionFile),
    Command(CommandActionFile),
}
```

Serde's `#[serde(tag = "kind")]` flattens the variant fields into the same object as `kind`. A `Pool` action serializes as `{"kind": "Pool", "instructions": ..., "pool": ..., "root": ..., "timeout": ...}`.

### Resolved types (`crates/barnum_config/src/resolved.rs:66-99`)

```rust
pub struct PoolAction {
    pub instructions: String,
    pub pool: Option<String>,
    pub root: Option<PathBuf>,
    pub timeout: Option<u64>,
}

pub struct CommandAction {
    pub script: String,
}

#[serde(tag = "kind")]
pub enum ActionKind {
    Pool(PoolAction),
    Command(CommandAction),
}
```

Same flattened pattern in the resolved types.

### Generated schemas

The Zod schema uses `z.discriminatedUnion("kind", [...])` with all action fields at the top level alongside `kind`:

```typescript
const ActionFile = z.discriminatedUnion("kind", [
  z.object({
    instructions: MaybeLinked_for_String.describe("..."),
    kind: z.literal("Pool"),
    pool: z.string().optional().describe("..."),
    root: z.string().optional().describe("..."),
    timeout: z.number().optional().describe("..."),
  }).strict(),
  z.object({
    kind: z.literal("Command"),
    script: z.string().describe("..."),
  }).strict(),
]);
```

### FinallyHook (`config.rs:198-203`)

```rust
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum FinallyHook {
    Command(HookCommand),
}
```

Same pattern. `{"kind": "Command", "script": "..."}` has `script` at the top level.

## Proposed Changes

### 1. Change serde representation to `tag` + `content`

**File:** `crates/barnum_config/src/config.rs`

Replace `#[serde(tag = "kind")]` with `#[serde(tag = "kind", content = "params")]`:

```rust
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "params")]
pub enum ActionFile {
    Pool(PoolActionFile),
    Command(CommandActionFile),
}
```

This changes the serialized form from:
```json
{"kind": "Pool", "instructions": "...", "pool": "..."}
```
to:
```json
{"kind": "Pool", "params": {"instructions": "...", "pool": "..."}}
```

`PoolActionFile` and `CommandActionFile` structs stay the same — they become the content of `params`.

### 2. Same change for resolved types

**File:** `crates/barnum_config/src/resolved.rs`

```rust
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind", content = "params")]
pub enum ActionKind {
    Pool(PoolAction),
    Command(CommandAction),
}
```

### 3. Same change for FinallyHook

**File:** `crates/barnum_config/src/config.rs`

```rust
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "params")]
pub enum FinallyHook {
    Command(HookCommand),
}
```

Before: `{"kind": "Command", "script": "..."}`
After: `{"kind": "Command", "params": {"script": "..."}}`

### 4. Update demo configs

Every `config.json` and `config.jsonc` in `crates/barnum_cli/demos/` needs the action fields wrapped in `params`. Example for a Pool action:

Before:
```json
{
  "action": {
    "kind": "Pool",
    "instructions": {"kind": "Inline", "value": "Analyze the given file."}
  }
}
```

After:
```json
{
  "action": {
    "kind": "Pool",
    "params": {
      "instructions": {"kind": "Inline", "value": "Analyze the given file."}
    }
  }
}
```

Example for a Command action:

Before:
```json
{
  "action": {
    "kind": "Command",
    "script": "jq '.value | {kind: \"Done\", value: .}' | jq -s"
  }
}
```

After:
```json
{
  "action": {
    "kind": "Command",
    "params": {
      "script": "jq '.value | {kind: \"Done\", value: .}' | jq -s"
    }
  }
}
```

Example for a finally hook:

Before:
```json
{"finally": {"kind": "Command", "script": "./finally-hook.sh"}}
```

After:
```json
{"finally": {"kind": "Command", "params": {"script": "./finally-hook.sh"}}}
```

### 5. Update test configs

All inline JSON configs in test files (`crates/barnum_config/tests/`, `crates/barnum_cli/tests/`) need the same wrapping. The `inject_pool_config` helpers in `tests/common/mod.rs` access `action.get("kind")` — they'll need to be updated since `kind` is still at the top level of the action object (that doesn't change), but the pool/root injection now goes into `action["params"]` instead of `action` directly.

**File:** `crates/barnum_config/tests/common/mod.rs`

Before:
```rust
if let Some(action) = step.get_mut("action")
    && action.get("kind").and_then(|k| k.as_str()) == Some("Pool")
{
    action["root"] = serde_json::json!(cli_root);
    action["pool"] = serde_json::json!(pool_name);
}
```

After:
```rust
if let Some(action) = step.get_mut("action")
    && action.get("kind").and_then(|k| k.as_str()) == Some("Pool")
    && let Some(params) = action.get_mut("params")
{
    params["root"] = serde_json::json!(cli_root);
    params["pool"] = serde_json::json!(pool_name);
}
```

Same change in `crates/barnum_cli/tests/common/mod.rs`.

### 6. Update docs generation

**File:** `crates/barnum_config/src/docs.rs`

`generate_step_docs` and `generate_full_docs` destructure `ActionKind` variants. The struct fields they access don't change — only the serde representation changes. Since docs generation works on resolved types (not JSON), no changes needed in docs.rs itself.

### 7. Update runner dispatch

**File:** `crates/barnum_config/src/runner/mod.rs`

`dispatch_task` destructures `ActionKind` variants. Same as docs — the Rust types don't change, only serde representation. No changes needed in the runner.

### 8. Regenerate schemas

Run `cargo run -p barnum_cli --bin build_schemas` to regenerate:
- `libs/barnum/barnum-config-schema.json`
- `libs/barnum/barnum-config-schema.zod.ts`
- `libs/barnum/barnum-cli-schema.zod.ts`

The Zod schema will change from `z.discriminatedUnion("kind", [...])` with flat objects to a structure that nests action-specific fields under `params`.

### 9. Update documentation

Files referencing the action config format:
- `crates/barnum_config/README.md` — config format examples
- `README.md` — quick start and example use cases
- `docs-website/docs/reference/cli.md` — CLI examples (if any inline configs)
- `docs-website/docs/repertoire/` — any patterns with inline configs
- `JS_ACTION_RESOLUTION.md` — action config examples throughout

## What doesn't change

- `PoolActionFile`, `CommandActionFile`, `HookCommand` struct definitions (fields stay the same)
- `PoolAction`, `CommandAction` resolved struct definitions
- All Rust code that pattern-matches on `ActionKind` or `ActionFile` (the Rust enum API is unchanged)
- `ConfigFile::resolve()` logic (it destructures enum variants, which work the same)
- State machine logic, validation, docs generation, runner dispatch
- The `kind` field stays at the top level of the action object (serde `tag` attribute)

## Scope

This is a config format change. Rust code that works with the typed enums doesn't change. The changes are:
1. Two serde attributes (3 lines of Rust)
2. All JSON config files (demos, tests)
3. Two `inject_pool_config` helpers
4. Generated schemas (automatic)
5. Documentation
