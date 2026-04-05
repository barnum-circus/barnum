# Handler Validation

**Blocked by:** `HANDLER_SCHEMAS_IN_AST.md` (which embeds JSON Schema in the AST)

## TL;DR

Once handler schemas are embedded in the AST (see `HANDLER_SCHEMAS_IN_AST.md`), compile them into validators on the Rust side and enforce them at runtime. Panic on validation failure.

---

## Task 1: Compile and validate schemas on Rust side

When the Rust binary deserializes the config, compile all JSON Schema documents into validators using the `jsonschema` crate. This serves two purposes: (a) validates that the schemas themselves are valid JSON Schema (catching malformed schemas at init, not at first handler invocation), and (b) caches compiled validators for use in Task 2.

### 1.1 Add `jsonschema` crate dependency

**File:** `crates/barnum_event_loop/Cargo.toml`

```toml
[dependencies]
jsonschema = "0.28"  # or latest
```

### 1.2 Compile schemas at workflow init, panic on invalid schema

**File:** `crates/barnum_event_loop/src/lib.rs`

```rust
use jsonschema::Validator;
use std::collections::HashMap;

struct CompiledSchemas {
    input: HashMap<HandlerId, Validator>,
    output: HashMap<HandlerId, Validator>,
}

fn compile_schemas(workflow_state: &WorkflowState) -> CompiledSchemas {
    let mut input = HashMap::new();
    let mut output = HashMap::new();
    for (handler_id, handler) in workflow_state.handlers() {
        if let HandlerKind::TypeScript(ts) = handler {
            if let Some(ref schema) = ts.input_schema {
                let validator = Validator::new(schema).unwrap_or_else(|err| {
                    panic!(
                        "invalid input JSON Schema for {}:{}: {err}",
                        ts.module.lookup(), ts.func.lookup(),
                    )
                });
                input.insert(handler_id, validator);
            }
            if let Some(ref schema) = ts.output_schema {
                let validator = Validator::new(schema).unwrap_or_else(|err| {
                    panic!(
                        "invalid output JSON Schema for {}:{}: {err}",
                        ts.module.lookup(), ts.func.lookup(),
                    )
                });
                output.insert(handler_id, validator);
            }
        }
    }
    CompiledSchemas { input, output }
}
```

Call `compile_schemas` at the top of `run_workflow`, before the main loop. Any invalid schema panics immediately with a clear message identifying the handler.

---

## Task 2: Validate handler inputs and outputs at runtime

Use the compiled schemas from Task 1 to validate values at the handler boundary. Panic on failure.

### 2.1 Validate input before dispatch

**File:** `crates/barnum_event_loop/src/lib.rs`

In the `run_workflow` dispatch loop:

```rust
for dispatch in &dispatches {
    let handler = workflow_state.handler(dispatch.handler_id);
    // TODO: elide redundant validation when adjacent schemas match
    if let Some(validator) = compiled_schemas.input.get(&dispatch.handler_id) {
        let errors: Vec<_> = validator.iter_errors(&dispatch.value).collect();
        if !errors.is_empty() {
            panic!(
                "input validation failed for {}:{}:\n{}",
                ts.module.lookup(), ts.func.lookup(),
                format_validation_errors(&errors),
            );
        }
    }
    scheduler.dispatch(dispatch, handler);
}
```

### 2.2 Validate output after completion

**File:** `crates/barnum_event_loop/src/lib.rs`

After receiving a handler result, before passing to `complete()`:

```rust
let value = result?;

// Look up which handler produced this task.
// task_to_frame maps TaskId → FrameId, frame has handler_id.
// We need to read handler_id BEFORE complete() removes the frame.
let handler_id = workflow_state.handler_id_for_task(task_id);

// TODO: elide redundant validation when adjacent schemas match
if let Some(validator) = compiled_schemas.output.get(&handler_id) {
    let errors: Vec<_> = validator.iter_errors(&value).collect();
    if !errors.is_empty() {
        let handler = workflow_state.handler(handler_id);
        panic!(
            "output validation failed for {}:{}:\n{}",
            handler.module(), handler.func(),
            format_validation_errors(&errors),
        );
    }
}

if let Some(terminal_value) = workflow_state.complete(task_id, value)? {
    return Ok(terminal_value);
}
```

Note: `handler_id_for_task` is a new method on `WorkflowState` that reads from `task_to_frame` without removing the entry. Currently `complete()` does `task_to_frame.remove()`. We need to read the handler_id first (via `task_to_frame.get()` → frame → handler_id) before `complete()` consumes it.

### 2.3 `format_validation_errors` helper

```rust
fn format_validation_errors(errors: &[jsonschema::ValidationError]) -> String {
    errors
        .iter()
        .map(|e| format!("  - {}: {}", e.instance_path, e))
        .collect::<Vec<_>>()
        .join("\n")
}
```

---

## Future optimization: eliding redundant validation

When two handlers are adjacent in a chain and the first handler's output schema is identical to the second handler's input schema, the output validation of the first and input validation of the second are redundant — one of them can be skipped. More generally, if a value flows through builtins (which are trusted) between two handlers with matching schemas, the intermediate validation can be elided.

This is a pure optimization and not part of this refactor. Implement naive validate-everything first, then add schema equality checks to skip redundant passes. Leave `// TODO: elide redundant validation when adjacent schemas match` comments at the validation call sites.

## What this does NOT include

- **No `Validate` builtin.** Validation is not a user-composable action in the pipeline. It's automatic enforcement at the handler boundary.
- **No handler deduplication / handler IDs.** Future work. For now, schemas are duplicated if the same handler appears multiple times in the AST.
- **No recovery from validation failures.** Validation failure = panic = workflow terminates. This is a contract violation, not an expected error. If we later want softer behavior (Result-based), that's a separate design.
- **No validation for builtins.** Builtins are framework code with known types. They don't cross a trust boundary.
- **No type-only handlers (specifying types without validators).** Separate refactor. For now, if you want runtime validation, provide a Zod schema. If you don't provide one, no validation occurs and the type defaults to `unknown` / `never` per existing behavior.

## Dependencies

| Package | Where | Purpose |
|---------|-------|---------|
| `jsonschema` (crate) | `barnum_event_loop` | Validate values against JSON Schema at runtime |
