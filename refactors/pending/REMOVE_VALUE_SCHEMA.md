# Remove Value Schema

**Parent:** JS_ACTION_RESOLUTION.md
**Depends on:** Nothing (independent Rust cleanup)

## Motivation

`value_schema` adds JSON Schema validation for task payloads at step boundaries. In practice, the action handlers already validate their own params via Zod schemas, and value validation at the barnum orchestration layer is unnecessary complexity. Removing it simplifies the config format, the resolved types, and the runtime.

## What Gets Removed

### Config file format

The `value_schema` field on steps — both inline and linked variants:

```json
{
  "steps": [{
    "name": "Analyze",
    "value_schema": { "type": "object", "properties": { "file": { "type": "string" } } },
    "action": { ... }
  }]
}
```

This removes `SchemaRef` (the `Inline`/`Link` union), `SchemaLink`, and the `resolve_schema()` function from `config.rs`.

### Resolved types

`Step.value_schema: Option<serde_json::Value>` in `resolved.rs`.

### Runtime validation

The entire `value_schema.rs` module:
- `CompiledSchemas` — compiles JSON schemas into validators at startup
- `CompiledSchemas::validate()` — validates task values against step schemas
- `ValidationError` — schema violation error type
- `validate_response()` — validates agent responses (schema + transition checks)
- `ResponseValidationError` — response validation error type

`validate_response()` also checks transition validity (`current_step.next.contains(&task.step)`). That transition check must be preserved — it moves into the caller or becomes a standalone function.

### `Task` struct

`Task` currently lives in `value_schema.rs`. It needs to move elsewhere (probably `types.rs` or its own module) since it's used throughout the runner.

### Docs generation

`generate_step_docs()` in `docs.rs` shows value schemas for next steps in agent instructions. Without value_schema, every next step just says "Accepts any JSON value."

### Dependencies

The `jsonschema` crate can be removed from `Cargo.toml` if no other code uses it.

## Files Changed

| File | Change |
|------|--------|
| `crates/barnum_config/src/value_schema.rs` | Delete entirely |
| `crates/barnum_config/src/config.rs` | Remove `value_schema` field from `StepFile`, remove `SchemaRef`, `SchemaLink`, `resolve_schema()` |
| `crates/barnum_config/src/resolved.rs` | Remove `value_schema` field from `Step` |
| `crates/barnum_config/src/docs.rs` | Remove `value_schema` branch in `generate_step_docs()` — always "Accepts any JSON value" |
| `crates/barnum_config/src/lib.rs` | Remove `mod value_schema`, remove `pub use value_schema::{CompiledSchemas, Task}` |
| `crates/barnum_config/src/runner/mod.rs` | Remove `CompiledSchemas` from imports and `run()`/`resume()` signatures |
| `crates/barnum_config/src/runner/response.rs` | Remove schema validation from `process_stdout()`, keep transition validation |
| `crates/barnum_cli/src/main.rs` | Remove `CompiledSchemas::compile()` call, update `run()`/`resume()` calls |
| `crates/barnum_config/Cargo.toml` | Remove `jsonschema` dependency |
| All test files | Remove `CompiledSchemas::compile()` calls, update `run()` call signatures |
| All demo configs | Remove `value_schema` fields |
| JSON schema + Zod schema | Regenerate (removes `value_schema` from config schema) |

## Transition Validation

`validate_response()` does two things: schema validation and transition validation. When removing schema validation, transition validation must be preserved. The cleanest approach: extract the transition check into a standalone function in `response.rs` or `value_schema.rs`'s replacement.

```rust
/// Validate an agent's response: check format and transition validity.
pub fn validate_response(
    response: &serde_json::Value,
    current_step: &Step,
) -> Result<Vec<Task>, ResponseValidationError> {
    // Same logic minus the schemas.validate() call
}
```

The `CompiledSchemas` parameter drops from the signature entirely.
