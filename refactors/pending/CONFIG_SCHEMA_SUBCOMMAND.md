# Config Schema Subcommand

**Status:** Not started

## Motivation

Users need a way to validate their config files and get IDE autocomplete. A `gsd schema` subcommand that outputs the JSON schema enables:

1. **Validation**: `gsd schema | ajv validate -s /dev/stdin -d config.jsonc`
2. **IDE integration**: Point VSCode/IntelliJ at the schema for autocomplete
3. **Documentation**: Schema serves as authoritative reference for config format

## Current State

- Config is defined in `crates/gsd_config/src/config.rs` using serde
- No way to extract the schema programmatically
- Users must read code or examples to understand config format

## Proposed Changes

### 1. Add schemars dependency

Add `schemars` crate to derive JSON Schema from Rust types:

```toml
# crates/gsd_config/Cargo.toml
[dependencies]
schemars = "0.8"
```

### 2. Derive JsonSchema on config types

```rust
// crates/gsd_config/src/config.rs
use schemars::JsonSchema;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct Config {
    // ...
}
```

### 3. Add schema subcommand to CLI

```rust
// crates/gsd_cli/src/main.rs
Command::Schema { pretty } => {
    let schema = schemars::schema_for!(Config);
    if pretty {
        println!("{}", serde_json::to_string_pretty(&schema)?);
    } else {
        println!("{}", serde_json::to_string(&schema)?);
    }
}
```

### 4. Add --json-schema flag to validate subcommand (optional)

Output validation errors in JSON format for tooling integration.

## Open Questions

1. Should we output draft-07 or draft-2020-12 schema?
2. Include examples in schema annotations?
3. Generate schema at build time and embed, or generate at runtime?

## Files to Change

- `crates/gsd_config/Cargo.toml` - add schemars
- `crates/gsd_config/src/config.rs` - derive JsonSchema
- `crates/gsd_cli/src/main.rs` - add schema subcommand
