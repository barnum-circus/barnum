# Config Validation (Rust Side)

## Motivation

The Rust evaluator currently trusts the incoming Config JSON without validation. `barnum_cli` deserializes the JSON into `barnum_ast::Config` via serde and immediately starts executing. If the TypeScript layer produces a malformed config (e.g., a `Step` referencing a nonexistent named step, or a handler with invalid stepConfig), the Rust side discovers the problem mid-execution with an opaque panic.

Validation should happen once, upfront, before any handler is invoked. The evaluator should receive a config that is structurally guaranteed to be valid.

## Current state

`crates/barnum_cli/src/main.rs` deserializes and re-serializes the config. No validation beyond what serde provides (correct JSON structure, correct `kind` tags).

`crates/barnum_ast/src/lib.rs` defines pure data types with no validation methods. The `Config` struct has `workflow: Action` and `steps: HashMap<String, Action>`.

`TypeScriptHandler` carries `step_config: Option<Value>` and `value_schema: Option<Value>`, but neither is validated against anything.

## Validations

### 1. All Step references resolve

Every `Action::Step { step }` node in the entire config (in `workflow` and in all step bodies) must reference a key that exists in `config.steps`.

Collect all Step references by walking the AST. Compare against `config.steps.keys()`. Report every unresolved reference with its location in the AST (e.g., "Step 'Cleanup' referenced in steps.Review.actions[2] but not defined").

### 2. No unreachable steps

Every key in `config.steps` must be reachable from `config.workflow`. A step is reachable if it's referenced by a `Step` node in the workflow or in another reachable step's action tree.

Walk `config.workflow` and all transitively reachable step bodies, collecting visited step names. Any step in `config.steps` not in the visited set is unreachable. Report them.

This is a warning rather than a hard error — unreachable steps are wasteful but not incorrect. However, they likely indicate a bug (typo in step name, forgotten step reference).

### 3. stepConfig validates against valueSchema (JSON Schema)

The TypeScript layer serializes each handler's `inputValidator` as a JSON Schema in the `valueSchema` field. If a `TypeScriptHandler` has both `step_config` and a schema for that config, validate `step_config` against the schema at config load time.

This catches config typos (wrong field names, wrong types) before any handler runs. The alternative — discovering bad config when the handler's zod validator rejects it at runtime — gives worse error messages and wastes execution.

Implementation: use `jsonschema` crate (or similar) for JSON Schema validation. The `valueSchema` field on `TypeScriptHandler` is the schema; `step_config` is the instance to validate.

Open question: should we also add a `stepConfigSchema` field to `TypeScriptHandler` alongside `valueSchema`? Currently `valueSchema` describes the handler's input (step value), not its config. stepConfig validation requires a separate schema. The TypeScript side would need to serialize `stepConfigValidator` to JSON Schema and include it in the serialized handler.

### 4. Structural invariants

These are constraints that serde can't enforce:

- **Sequence has at least 1 action.** An empty sequence is meaningless.
- **All has at least 2 actions.** A single-branch All is just the action itself.
- **Match has at least 1 case.** An empty match has no valid input.
- **Loop body is present.** Already enforced by serde (body is required), but worth a redundant check.
- **Call handler module path is non-empty.** An empty module path can't resolve.
- **Call handler func name is non-empty.** An empty function name can't resolve.

### 5. Step(Root) only appears in the workflow, not in defined steps

`Step(Root)` (self-recursion to the workflow root) is only meaningful inside the workflow tree. If it appears in a defined step body, it's a bug — the step has no way to know the workflow root, and the flattener resolves `Step(Root)` using the workflow root ActionId which is only set when flattening the workflow.

Report: `"Step(Root) found in step '{name}' — Step(Root) is only valid in the workflow tree"`

### 6. No direct recursion in steps

A step whose body is just `Step(itself)` is an infinite loop with no useful work. More generally, cycles through steps are valid (mutual recursion is a feature), but a step that directly references itself as its entire body (not within a loop or match) is almost certainly a bug.

This one is tricky to define precisely — `Step("A")` where A's body is `Sequence([..., Step("A")])` is a legitimate recursive pattern (the sequence does work before recurring). Only flag the degenerate case where the step body IS just `Step("self")`.

### 7. Handler module paths exist on disk

If the handler kind is `TypeScript`, verify that the `module` path exists on the filesystem. This catches path typos before execution. Requires access to the filesystem, so this validation belongs in the CLI or evaluator, not in the pure AST crate.

## Architecture

Validation belongs in a new `validate` function, not in serde deserialization. Serde handles structural parsing; validation is a separate pass.

```rust
// In barnum_ast or a new barnum_validate crate
pub fn validate(config: &Config) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();
    validate_step_references(config, &mut errors);
    validate_reachability(config, &mut errors);
    validate_structural_invariants(config, &mut errors);
    // ...
    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}
```

Collecting all errors rather than failing on the first one gives a better developer experience.

`ValidationError` should be a structured enum with enough context to produce good messages:

```rust
pub enum ValidationError {
    UnresolvedStep { step_name: String, location: String },
    UnreachableStep { step_name: String },
    EmptySequence { location: String },
    EmptyModule { location: String },
    // ...
}
```

The `location` field is a human-readable path like "workflow.actions[2].cases.HasErrors.actions[0]" that points to the problematic node.

## Open questions

1. Should filesystem checks (handler module path exists) be part of the same validation pass, or a separate phase? The pure structural checks don't need I/O; filesystem checks do.

2. Should unreachable steps be an error or a warning? Warnings require a separate reporting mechanism (the current signature returns `Result<(), Vec<ValidationError>>`).

3. Should we validate that Match case keys are non-empty strings? Technically valid JSON but likely a bug.

4. The `stepConfig` JSON Schema validation requires the TypeScript side to serialize validators as JSON Schema. Is that already happening, or does it need to be added to `fromConfig`?
