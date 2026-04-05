# Handler Validation

**Blocked by:** `HANDLER_SCHEMAS_IN_AST.md` (done)

## TL;DR

Compile embedded JSON Schemas into validators at workflow init using the `jsonschema` crate. Validate handler inputs before dispatch and outputs after completion. Validation failures are `RunWorkflowError` variants propagated with `?` — no panics at the validation site.

---

## Design notes

**Why `JsonSchema(Value)` stays as-is.** The `jsonschema` crate takes `&Value` for both schema compilation and value validation. A typed Rust enum for JSON Schema would require converting back to `Value` for the actual validation — a round-trip for no runtime benefit. `Validator::new()` already validates that the schema is well-formed JSON Schema during compilation, so malformed schemas are caught at init time regardless.

**Error handling.** All validation failures propagate as `Result::Err` via `?`. The caller of `run_workflow` decides whether to panic, log, or handle otherwise. Inside the event loop, validation errors are just another `RunWorkflowError` variant.

---

## Step 1: Add `jsonschema` dependency

**File:** `crates/barnum_event_loop/Cargo.toml`

```toml
[dependencies]
jsonschema = "0.28"  # or latest
```

---

## Step 2: Add validation error variants to `RunWorkflowError`

**File:** `crates/barnum_event_loop/src/lib.rs`

```rust
pub enum RunWorkflowError {
    Handler(HandlerError),
    Complete(CompleteError),

    /// A handler's embedded JSON Schema is not valid JSON Schema.
    /// Caught at workflow init during schema compilation.
    InvalidSchema {
        module: String,
        func: String,
        direction: SchemaDirection,
        error: String,
    },

    /// A handler's input or output value failed schema validation.
    SchemaValidation {
        module: String,
        func: String,
        direction: SchemaDirection,
        errors: Vec<String>,
    },
}

/// Whether a schema/validation error is about the handler's input or output.
pub enum SchemaDirection {
    Input,
    Output,
}
```

Implement `Display` for `SchemaDirection` (`"input"` / `"output"`) so error messages read naturally: `"input validation failed for /path/to/module:funcName"`.

---

## Step 3: Compile schemas at workflow init

**File:** `crates/barnum_event_loop/src/lib.rs`

At the top of `run_workflow`, before the main loop, iterate all handlers and compile their schemas. Invalid schemas return `Err(RunWorkflowError::InvalidSchema(...))`.

```rust
use jsonschema::Validator;

/// Compiled input/output validators, keyed by flat action index.
/// Only TypeScript handlers with schemas have entries.
struct CompiledSchemas {
    /// flat action index → compiled input validator
    input: HashMap<usize, Validator>,
    /// flat action index → compiled output validator
    output: HashMap<usize, Validator>,
}

fn compile_schemas(
    workflow_state: &WorkflowState,
) -> Result<CompiledSchemas, RunWorkflowError> {
    let mut input = HashMap::new();
    let mut output = HashMap::new();

    for (index, flat_entry) in workflow_state.flat_entries().iter().enumerate() {
        if let FlatAction::Invoke { handler } = &flat_entry.action {
            if let HandlerKind::TypeScript(ts_handler) = handler {
                if let Some(ref schema) = ts_handler.input_schema {
                    let validator = Validator::new(&schema.0).map_err(|err| {
                        RunWorkflowError::InvalidSchema {
                            module: ts_handler.module.to_string(),
                            func: ts_handler.func.to_string(),
                            direction: SchemaDirection::Input,
                            error: err.to_string(),
                        }
                    })?;
                    input.insert(index, validator);
                }
                if let Some(ref schema) = ts_handler.output_schema {
                    let validator = Validator::new(&schema.0).map_err(|err| {
                        RunWorkflowError::InvalidSchema {
                            module: ts_handler.module.to_string(),
                            func: ts_handler.func.to_string(),
                            direction: SchemaDirection::Output,
                            error: err.to_string(),
                        }
                    })?;
                    output.insert(index, validator);
                }
            }
        }
    }

    Ok(CompiledSchemas { input, output })
}
```

The exact iteration API depends on how `WorkflowState` exposes flat entries. The key is: iterate all `Invoke` actions with `TypeScript` handlers, compile their schemas, key by whatever identifier the dispatch/completion paths use to look up the handler.

Call at the top of `run_workflow`:

```rust
let compiled_schemas = compile_schemas(workflow_state)?;
```

---

## Step 4: Validate at handler boundaries

### 4.1 Validate input before dispatch

In `run_workflow`, before dispatching to the scheduler:

```rust
// Before scheduler.dispatch(...)
if let HandlerKind::TypeScript(ts_handler) = handler {
    if let Some(validator) = compiled_schemas.input.get(&dispatch_index) {
        let errors: Vec<_> = validator.iter_errors(&dispatch_event.value).collect();
        if !errors.is_empty() {
            return Err(RunWorkflowError::SchemaValidation {
                module: ts_handler.module.to_string(),
                func: ts_handler.func.to_string(),
                direction: SchemaDirection::Input,
                errors: format_validation_errors(&errors),
            });
        }
    }
}
// TODO: elide redundant validation when adjacent schemas match
```

### 4.2 Validate output after handler completion

After receiving a handler result, before passing to `complete()`:

```rust
let value = result.map_err(RunWorkflowError::Handler)?;

// Look up which handler produced this result BEFORE complete() removes the frame.
// Need the handler info + flat action index for the validator lookup.
let (handler_kind, action_index) = workflow_state.handler_for_task(task_id);

if let HandlerKind::TypeScript(ts_handler) = handler_kind {
    if let Some(validator) = compiled_schemas.output.get(&action_index) {
        let errors: Vec<_> = validator.iter_errors(&value).collect();
        if !errors.is_empty() {
            return Err(RunWorkflowError::SchemaValidation {
                module: ts_handler.module.to_string(),
                func: ts_handler.func.to_string(),
                direction: SchemaDirection::Output,
                errors: format_validation_errors(&errors),
            });
        }
    }
}
// TODO: elide redundant validation when adjacent schemas match

let completion_event = CompletionEvent { task_id, value };
if let Some(terminal_value) = complete(workflow_state, completion_event)? {
    return Ok(terminal_value);
}
```

Note: `handler_for_task` is a new method that reads from `task_to_frame` (via `.get()`, not `.remove()`) to get the handler kind and action index before `complete()` consumes the frame.

### 4.3 Error formatting helper

```rust
fn format_validation_errors(errors: &[jsonschema::ValidationError]) -> Vec<String> {
    errors
        .iter()
        .map(|e| format!("{}: {}", e.instance_path, e))
        .collect()
}
```

---

## What this does NOT include

- **No `Validate` builtin.** Validation is automatic at the handler boundary, not a user-composable pipeline action.
- **No handler deduplication.** If the same handler appears multiple times in the AST, its schema is compiled multiple times. Future optimization.
- **No validation for builtins.** Builtins are framework code with known types. No trust boundary.
- **No redundant validation elision.** Validate everything naively first. When two adjacent handlers share a schema, one validation can be skipped — that's a future optimization. `// TODO` comments mark the sites.
- **No recovery from validation failures.** Validation failure terminates the workflow via `RunWorkflowError`. This is a contract violation, not a retryable error.

## Dependencies

| Package | Where | Purpose |
|---------|-------|---------|
| `jsonschema` (crate) | `barnum_event_loop` | Compile and validate JSON Schema at runtime |
