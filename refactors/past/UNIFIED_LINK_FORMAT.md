# Unified Link Format

**Status:** Not started

**Depends on:** Nothing (can be done independently, before INLINED_CONFIG)

**Blocks:** INLINED_CONFIG (uses this unified format)

## Motivation

Currently, `SchemaRef` and `Instructions` use different formats for file links:

| Type | Link Format | Example |
|------|-------------|---------|
| `Instructions` | `{"link": "path"}` | `{"link": "analyze.md"}` |
| `SchemaRef` | bare string | `"schemas/order.json"` |

This inconsistency:
1. Makes the config format harder to learn
2. Requires different deserialization logic for each type
3. Prevents using a generic `MaybeLinked<T>` type (see INLINED_CONFIG)

## Goal

Unify both to use `{"link": "path"}` format:

| Type | Before | After |
|------|--------|-------|
| `Instructions` | `{"link": "path"}` | `{"link": "path"}` (no change) |
| `SchemaRef` | `"path"` | `{"link": "path"}` |

**Breaking change:** Yes. Schema links change from bare strings to objects.

## Locations to Change

### 1. Rust Code

#### config.rs:220-232 - SchemaRef enum

**File:** `crates/gsd_config/src/config.rs`

```rust
// Before (line 220-232)
/// Reference to a JSON Schema (inline or external file).
///
/// In config files:
/// - String → link to schema file
/// - Object → inline JSON Schema
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SchemaRef {
    /// Path to a JSON Schema file.
    Link(String),
    /// Inline JSON Schema.
    Inline(serde_json::Value),
}

// After
/// Reference to a JSON Schema (inline or external file).
///
/// In config files:
/// - `{"link": "path"}` → link to schema file
/// - Object → inline JSON Schema
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SchemaRef {
    /// Link to a JSON Schema file.
    Link {
        /// Path to the schema file.
        link: String,
    },
    /// Inline JSON Schema.
    Inline(serde_json::Value),
}
```

#### value_schema.rs:32-52 - Pattern match in compile()

**File:** `crates/gsd_config/src/value_schema.rs`

```rust
// Before (line 29-54)
for step in &config.steps {
    let validator = match &step.value_schema {
        None => None,
        Some(SchemaRef::Inline(schema)) => Some(compile_schema(schema)?),
        Some(SchemaRef::Link(path)) => {
            let full_path = base_path.join(path);
            // ...
        }
    };
    // ...
}

// After
for step in &config.steps {
    let validator = match &step.value_schema {
        None => None,
        Some(SchemaRef::Inline(schema)) => Some(compile_schema(schema)?),
        Some(SchemaRef::Link { link }) => {
            let full_path = base_path.join(link);
            // ...
        }
    };
    // ...
}
```

#### docs.rs:103 - Pattern match in generate_step_docs()

**File:** `crates/gsd_config/src/docs.rs`

```rust
// Before (line 89-110)
match &next_step.value_schema {
    None => { /* ... */ }
    Some(SchemaRef::Inline(schema)) => { /* ... */ }
    Some(SchemaRef::Link(path)) => {
        writeln!(doc, "Value must match schema in `{path}`.").ok();
        // ...
    }
}

// After
match &next_step.value_schema {
    None => { /* ... */ }
    Some(SchemaRef::Inline(schema)) => { /* ... */ }
    Some(SchemaRef::Link { link }) => {
        writeln!(doc, "Value must match schema in `{link}`.").ok();
        // ...
    }
}
```

### 2. Tests

#### config.rs:636-650 - schema_link_string test

**File:** `crates/gsd_config/src/config.rs`

```rust
// Before (line 635-650)
#[test]
fn schema_link_string() {
    let json = r#"{
        "steps": [{
            "name": "Test",
            "value_schema": "schemas/test.json",
            "next": []
        }]
    }"#;

    let config: Config = serde_json::from_str(json).expect("parse failed");
    assert!(matches!(
        &config.steps[0].value_schema,
        Some(SchemaRef::Link(path)) if path == "schemas/test.json"
    ));
}

// After
#[test]
fn schema_link_object() {
    let json = r#"{
        "steps": [{
            "name": "Test",
            "value_schema": {"link": "schemas/test.json"},
            "next": []
        }]
    }"#;

    let config: Config = serde_json::from_str(json).expect("parse failed");
    assert!(matches!(
        &config.steps[0].value_schema,
        Some(SchemaRef::Link { link }) if link == "schemas/test.json"
    ));
}
```

### 3. Documentation

#### docs/recipes/validation.md:66 - External schema example

**File:** `docs/recipes/validation.md`

```json
// Before (line 62-78)
{
  "steps": [
    {
      "name": "ProcessOrder",
      "value_schema": "schemas/order.json",
      "action": { "kind": "Pool", "instructions": "Process the order..." },
      "next": ["Ship"]
    },
    // ...
  ]
}

// After
{
  "steps": [
    {
      "name": "ProcessOrder",
      "value_schema": {"link": "schemas/order.json"},
      "action": { "kind": "Pool", "instructions": "Process the order..." },
      "next": ["Ship"]
    },
    // ...
  ]
}
```

### 4. README

#### crates/gsd_config/README.md - Config format example

**File:** `crates/gsd_config/README.md`

```json
// Before (line 49)
"value_schema": { "link": "implement-instructions.md" }  // Actually this is instructions not schema!
```

Note: The README example already uses the `{"link": ...}` format for instructions. There's no example of linked schemas in the README.

### 5. Demo Configs

**None affected.** All demo configs use inline schemas, not linked schemas:

```bash
$ grep -r '"value_schema":' crates/gsd_cli/demos/
# All results show inline schemas like {"type": "object", ...}
```

### 6. JSON Schema (Auto-generated)

The JSON schema at `crates/gsd_config/schemas/config.schema.json` is auto-generated by `schemars` from the Rust types (via `gsd config schema`). It will automatically reflect the new format after the Rust types are changed.

No manual changes needed, but the schema output will change from:

```json
// Before (schemars output for untagged enum with Link(String))
"SchemaRef": {
  "anyOf": [
    { "type": "string" },
    { "type": "object" }
  ]
}

// After (schemars output for untagged enum with Link { link: String })
"SchemaRef": {
  "anyOf": [
    {
      "type": "object",
      "required": ["link"],
      "properties": {
        "link": { "type": "string" }
      }
    },
    { "type": "object" }
  ]
}
```

## Implementation

This is a small, focused change:

1. Update `SchemaRef` enum in `config.rs`
2. Update pattern matches in `value_schema.rs` and `docs.rs`
3. Rename and update the test
4. Update the documentation example

**Estimated: ~20 lines of code changes, plus documentation.**

## Verification

After implementation:

1. `cargo test -p gsd_config` passes
2. `cargo test -p gsd_cli` passes (demos run)
3. Documentation example is valid JSON
