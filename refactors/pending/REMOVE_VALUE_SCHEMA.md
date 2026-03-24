# Remove Value Schema

**Parent:** JS_ACTION_RESOLUTION.md
**Depends on:** Nothing (independent Rust cleanup)

## Motivation

`value_schema` adds JSON Schema validation for task payloads at step boundaries. Each action handler already validates its own params via Zod schemas (`stepConfigurationSchema` / `stepParameterSchema`), and value validation at the barnum orchestration layer is unnecessary complexity. Removing it simplifies the config format, the resolved types, the runtime, and eliminates the `jsonschema` crate dependency.

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

This field is `Option<SchemaRef>` where `SchemaRef` is an untagged union of inline JSON Schema or a file link:

```rust
// config.rs:270-287
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SchemaRef {
    Link(SchemaLink),
    Inline(serde_json::Value),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SchemaLink {
    pub link: String,
}
```

All of `SchemaRef`, `SchemaLink`, and `resolve_schema()` (config.rs:449-470) are deleted.

### Resolved types

**File:** `crates/barnum_config/src/resolved.rs`

Remove `value_schema` field from `Step`:

```rust
// Before
pub struct Step {
    pub name: StepName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_schema: Option<serde_json::Value>,  // DELETE
    pub action: ActionKind,
    pub next: Vec<StepName>,
    // ...
}

// After
pub struct Step {
    pub name: StepName,
    pub action: ActionKind,
    pub next: Vec<StepName>,
    // ...
}
```

### Runtime validation — `value_schema.rs`

**File:** `crates/barnum_config/src/value_schema.rs` — delete entirely (357 lines).

This module contains:

**`CompiledSchemas`** (lines 11-69) — compiles each step's JSON Schema into a `jsonschema::Validator` at startup, stores them in a `HashMap<StepName, Option<Validator>>`. Called once in `main.rs` and once in `resume()`.

**`ValidationError`** (lines 80-109) — error type for schema violations. Two variants: `UnknownStep` and `SchemaViolation`.

**`Task`** (lines 111-134) — the runtime task struct with `step: StepName`, `value: StepInputValue`, and `retries: u32`. This struct must be preserved — it's used throughout the runner. Move it to `types.rs`.

**`validate_response()`** (lines 136-184) — validates agent responses. Does three things:
1. Checks response is a JSON array
2. Checks each task's `kind` is a valid next step (transition validation)
3. Validates each task's `value` against the target step's schema

Items 1 and 2 must be preserved. Item 3 is removed.

**`ResponseValidationError`** (lines 186-238) — error type for response validation. Four variants: `NotAnArray`, `InvalidTaskFormat`, `InvalidTransition`, `SchemaError`. The `SchemaError` variant is removed; the others are preserved.

**Unit tests** (lines 240-356) — 7 tests. Delete `validates_correct_value`, `rejects_invalid_value`, `accepts_any_value_without_schema`. Keep (and move) `validate_response_accepts_valid_array`, `validate_response_rejects_non_array`, `validate_response_rejects_invalid_transition`, `validate_response_accepts_empty_array`.

### Task struct relocation

`Task` is used in:
- `runner/mod.rs` — task creation, dispatch, state tracking
- `runner/response.rs` — response parsing
- `runner/action.rs` — worker result types
- `main.rs` — initial task creation
- All test files — `CompiledSchemas::compile` and `run()` calls

Move `Task` and `Task::new()` to `crates/barnum_config/src/types.rs`. Update the `pub use` in `lib.rs` from `value_schema::Task` to `types::Task`.

### Docs generation

**File:** `crates/barnum_config/src/docs.rs`

`generate_step_docs()` (lines 56-84) shows value schemas for next steps. Remove the `match &next_step.value_schema` branch. Every next step just shows "Accepts any JSON value."

```rust
// Before (docs.rs:60-83)
match &next_step.value_schema {
    None => {
        writeln!(doc, "Accepts any JSON value.").ok();
        // ...
    }
    Some(schema) => {
        writeln!(doc, "Value must match schema:").ok();
        // ...
    }
}

// After
writeln!(doc, "Accepts any JSON value.").ok();
writeln!(doc).ok();
writeln!(doc, "```json").ok();
writeln!(doc, r#"{{"kind": "{next_name}", "value": <any>}}"#).ok();
writeln!(doc, "```").ok();
```

### StepFile.resolve()

**File:** `crates/barnum_config/src/config.rs` (lines 386-416)

Remove the `value_schema` resolution from `StepFile::resolve()`:

```rust
// Before
fn resolve(self, base_path: &Path, global_options: &Options) -> io::Result<resolved::Step> {
    let action = self.action.resolve(base_path)?;
    let value_schema = self                    // DELETE
        .value_schema                          // DELETE
        .map(|s| resolve_schema(s, base_path)) // DELETE
        .transpose()?;                         // DELETE
    let options = EffectiveOptions::resolve(global_options, &self.options);

    Ok(resolved::Step {
        name: self.name,
        value_schema,  // DELETE
        action,
        // ...
    })
}
```

### Runner changes

**File:** `crates/barnum_config/src/runner/mod.rs`

Remove `CompiledSchemas` from function signatures. Currently `run()` and `resume()` take `&CompiledSchemas` as a parameter. After this change, they don't.

```rust
// Before
pub fn run(
    config: &Config,
    schemas: &CompiledSchemas,
    runner_config: &RunnerConfig,
    initial_tasks: Vec<Task>,
) -> io::Result<()>

// After
pub fn run(
    config: &Config,
    runner_config: &RunnerConfig,
    initial_tasks: Vec<Task>,
) -> io::Result<()>
```

Same for `resume()`.

**File:** `crates/barnum_config/src/runner/response.rs`

`process_stdout()` calls `validate_response(&output_value, step, schemas)`. After removing schema validation, the `schemas` parameter is gone:

```rust
// Before (response.rs:70)
validate_response(&output_value, step, schemas)

// After
validate_response(&output_value, step)
```

### CLI changes

**File:** `crates/barnum_cli/src/main.rs`

Remove `CompiledSchemas::compile(&cfg)?` call (line 181) and update `run()`/`resume()` call sites to drop the `schemas` argument.

```rust
// Before
let schemas = CompiledSchemas::compile(&cfg)?;
let initial_tasks = resolve_initial_tasks(&schemas, ...);
run(&cfg, &schemas, &runner_config, initial_tasks)?;

// After
let initial_tasks = resolve_initial_tasks(...);
run(&cfg, &runner_config, initial_tasks)?;
```

`resolve_initial_tasks` also needs updating — it currently validates initial task values against schemas. Without value_schema, it just parses the JSON array into `Vec<Task>` without schema validation.

### Cargo.toml

**File:** `crates/barnum_config/Cargo.toml`

Remove the `jsonschema` dependency. Verify no other code in the crate uses it.

### Test files

All 12 integration test files call `CompiledSchemas::compile(&config)` and pass `&schemas` to `run()`. Every test needs updating:

| Test file | Approximate `compile()` calls to remove |
|-----------|---------------------------------------|
| `branching_transitions.rs` | 3 |
| `linear_transitions.rs` | 2 |
| `invalid_transitions.rs` | 3 |
| `retry_behavior.rs` | 6 |
| `concurrency.rs` | 3 |
| `finally_retry_bugs.rs` | 13 |
| `simple_termination.rs` | 2 |
| `ordered_agent.rs` | 1 |
| `edge_cases.rs` | 1 (+ delete `invalid_value_schema_in_initial_tasks_returns_error` test) |
| `schema_validation.rs` | Delete entirely (all tests are schema-specific) |

The test helper `make_config_and_schemas()` in `main.rs` tests becomes `make_config()` — no schemas.

### Demo configs

All demo configs under `crates/barnum_cli/demos/*/config.json` and `config.jsonc` that have `value_schema` fields need them removed. Based on the current demos, most steps don't use `value_schema`, but check all of them.

### Generated schemas

Run `cargo run -p barnum_cli --bin build_schemas` after the changes. The generated files (`barnum-config-schema.json`, `barnum-config-schema.zod.ts`) will no longer include `value_schema` in the step definition.

## Transition Validation

`validate_response()` does three things. Only the schema validation is removed:

```rust
/// Validate an agent's response: check format and transition validity.
pub fn validate_response(
    response: &serde_json::Value,
    current_step: &Step,
) -> Result<Vec<Task>, ResponseValidationError> {
    let serde_json::Value::Array(items) = response else {
        return Err(ResponseValidationError::NotAnArray);
    };

    let mut tasks = Vec::with_capacity(items.len());

    for (i, item) in items.iter().enumerate() {
        let task: Task = serde_json::from_value(item.clone()).map_err(|e| {
            ResponseValidationError::InvalidTaskFormat {
                index: i,
                error: e.to_string(),
            }
        })?;

        // Check valid transition
        if !current_step.next.contains(&task.step) {
            return Err(ResponseValidationError::InvalidTransition {
                from: current_step.name.clone(),
                to: task.step,
                valid: current_step.next.clone(),
            });
        }

        // Schema validation removed — no schemas.validate() call

        tasks.push(task);
    }

    Ok(tasks)
}
```

This function moves from `value_schema.rs` (which is deleted) to `runner/response.rs` alongside `process_stdout()` which is its only caller. `ResponseValidationError` moves with it, minus the `SchemaError` variant.
