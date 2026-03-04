# GSD Config Schema Generation

**Status:** Not started

## Motivation

Users need a way to validate their GSD config files and get IDE autocomplete. JSON schema generation enables:

1. **Validation**: `gsd config validate config.jsonc`
2. **IDE integration**: Point VSCode/IntelliJ at the schema for autocomplete
3. **Documentation**: Schema serves as authoritative reference for config format

Note: agent_pool already has schema support for task payloads. This refactor focuses only on GSD config.

## Current State

- GSD config is defined in `crates/gsd_config/src/config.rs` using serde
- No way to extract the schema programmatically
- Users must read code or examples to understand config format

## Proposed Changes

### Command Structure

```bash
gsd config schema           # Print config JSON schema (pretty)
gsd config docs             # Print config schema with documentation
gsd config validate <file>  # Validate a config file against schema
```

### 1. Add schemars dependency

Add `schemars` crate to derive JSON Schema from Rust types (same as isograph):

```toml
# crates/gsd_config/Cargo.toml
[dependencies]
schemars = "0.8"
```

### 2. Add $schema field to config struct

Allow users to specify the schema in their config file (like isograph):

```rust
// crates/gsd_config/src/config.rs
use schemars::JsonSchema;

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// The user may hard-code the JSON Schema for their version of the config.
    #[serde(rename = "$schema")]
    pub json_schema: Option<String>,

    // ... existing fields ...
}
```

This allows config files to include:
```json
{
  "$schema": "https://unpkg.com/@gsd/cli@latest/gsd-config-schema.json",
  "tasks": [...]
}
```

### 3. Derive JsonSchema on all config types

All nested config types need `#[derive(JsonSchema)]`.

### 4. Create build-time schema generator binary

Following isograph's pattern, create a separate binary that generates the schema:

```rust
// crates/gsd_config/src/bin/build_json_schema.rs
use gsd_config::Config;
use schemars::schema_for;
use std::fs;

fn main() {
    let schema = schema_for!(Config);
    fs::write(
        "./libs/gsd-cli/gsd-config-schema.json",
        serde_json::to_string_pretty(&schema).unwrap(),
    ).unwrap();
}
```

### 5. Schema file location

Generate schema to `libs/gsd-cli/gsd-config-schema.json` for npm publishing.

Accessible via CDN:
```
https://unpkg.com/@gsd/cli@latest/gsd-config-schema.json
```

### 6. Add config subcommand to gsd CLI

```rust
#[derive(Subcommand)]
enum Commands {
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    // ... existing commands
}

#[derive(Subcommand)]
enum ConfigCommands {
    /// Print config JSON schema
    Schema,
    /// Print config schema with documentation
    Docs,
    /// Validate a config file against schema
    Validate { file: PathBuf },
}
```

### 7. Add CI check

Add a CI step that runs the schema generator:

```yaml
# .github/workflows/ci.yml
- name: Generate GSD config schema
  run: cargo run -p gsd_config --bin build_json_schema
```

Since we generate at build time and publish to npm, we don't need to check in the generated file. CI just verifies the generator runs without error.

## Files to Change

- `crates/gsd_config/Cargo.toml` - add schemars, add bin target
- `crates/gsd_config/src/config.rs` - derive JsonSchema, add $schema field
- `crates/gsd_config/src/bin/build_json_schema.rs` - new binary
- `crates/gsd_cli/src/main.rs` - add config subcommand
- `libs/gsd-cli/` - schema output location (npm package)
- `.github/workflows/ci.yml` - add schema generation check

## Usage with AI Agents

When asking an AI agent to create or modify GSD config files, tell them about the schema command:

```
When creating GSD config files, run `pnpm dlx @gsd-now/gsd config schema` to see the JSON schema for the config format. This will show you all available fields and their types.
```

This gives agents the authoritative reference for the config format without needing to explain it manually.

## README Updates

Update the README to:
1. Add a note at the top that examples use `pnpm dlx` but you can use `npx` or install globally
2. Mention `gsd config schema` as the way to discover the config format
3. Suggest telling AI agents about the schema command when asking them to create configs

## Design Decisions

1. **Use schemars (same as isograph)** - proven approach, works well
2. **$schema field with serde rename** - allows users to get IDE autocomplete by adding the field
3. **Generate at build time** - schema is static, no need for runtime generation
4. **Output to libs/ for npm** - makes schema accessible via CDN for IDE integration
5. **Punt on agent_pool** - it already has schema support
6. **AI agent friendly** - schema command provides authoritative reference for agents creating configs
