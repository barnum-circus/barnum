# Unify Stdin Envelope

**Depends on:** FLATTEN_AND_RENAME_ACTION
**Blocks:** ADD_TYPESCRIPT_DISPATCH

## Motivation

The stdin envelope is currently `{"kind": "StepName", "value": {...}}`. The target is `{"value": {...}, "config": {...}, "stepName": "StepName"}` — matching the `HandlerContext` shape so TypeScript handlers receive the envelope almost directly. ADD_TYPESCRIPT_DISPATCH later adds `stepConfig` for TypeScript actions.

## Current state

**File:** `crates/barnum_config/src/runner/action.rs:170-174`

```rust
let task_json = serde_json::to_string(&serde_json::json!({
    "kind": &self.step_name,
    "value": &value,
}))
.unwrap_or_default();
```

## Changes

### Envelope struct

**File:** `crates/barnum_config/src/runner/action.rs`

Replace the inline `json!` with a typed struct:

```rust
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Envelope<'a> {
    value: &'a serde_json::Value,
    config: &'a serde_json::Value,
    step_name: &'a StepName,
}
```

### ShellAction gains `config`

`ShellAction` stores `config: Arc<serde_json::Value>`. Engine serializes the config once at startup and shares via `Arc`.

### Demos and tests

No demo scripts read `.kind` from stdin — they only access `.value.*`. The integration tests use scripts that ignore stdin (`echo '[]'`). No changes needed.

### Documentation

Update `docs-website/docs/reference/task-format.md` to reflect the new envelope shape.
