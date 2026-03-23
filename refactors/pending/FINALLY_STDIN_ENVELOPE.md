# Unify Finally Hook Stdin Format

Sub-refactor extracted from UNIFIED_ACTION_DISPATCH.md (Phase 3 complication). Can land independently.

## Motivation

Command actions receive stdin in envelope format: `{"kind": "StepName", "value": {...}}`. Finally hooks receive stdin as raw value: `{...}` (no envelope). This inconsistency means scripts that handle both formats need different parsing logic, and when UNIFIED_ACTION_DISPATCH moves finally hooks through `ShellAction`, the stdin format changes silently.

Unifying before that refactor makes the format change explicit and independent. It also simplifies the mental model: every shell script barnum invokes receives the same envelope format.

## Current State

### `dispatch_finally_task` (`runner/dispatch.rs:107`)

```rust
let input_json = serde_json::to_string(&value.0).unwrap_or_default();
```

Passes raw `value.0` — just the JSON value with no wrapping.

### `dispatch_command_task` (`runner/dispatch.rs:80-84`)

```rust
let task_json = serde_json::to_string(&serde_json::json!({
    "kind": &task.step,
    "value": &value.0,
}))
.unwrap_or_default();
```

Wraps in `{"kind": "<step_name>", "value": <payload>}`.

### Documentation (`config.rs`)

Line 110-111 (Step.finally_hook doc comment):
```rust
/// **stdin:** The task's original `value` payload as JSON (same as what
/// the pre hook received — just the value, not the full task wrapper).
```

Lines 177 (FinallyHook doc comment):
```rust
/// **stdin:** The task's original value payload as JSON.
```

Line 146 (CommandActionFile.script doc comment):
```rust
/// **Input (stdin):** JSON object: `{"kind": "<step name>", "value": <payload>}`.
```

### Demo script (`demos/hooks/finally-hook.sh`)

```bash
item=$(echo "$input" | jq -r '.item')
```

Reads directly from the raw value — no `.value` prefix.

### Docs website

- `docs-website/docs/repertoire/hooks.md:79` — documents raw value format
- `docs-website/docs/repertoire/fan-out-finally.md:156` — documents raw value format
- `docs-website/docs/reference/config-schema.md:178-184` — documents raw value format

### Generated schema (`libs/barnum/barnum-config-schema.json:101`)

Description string says "original value payload as JSON" — generated from the doc comment on `FinallyHook` in `config.rs`.

## Changes

### 1. Update `dispatch_finally_task` stdin

**File: `crates/barnum_config/src/runner/dispatch.rs`**

Before:
```rust
let input_json = serde_json::to_string(&value.0).unwrap_or_default();
```

After:
```rust
let input_json = serde_json::to_string(&serde_json::json!({
    "kind": &task.step,
    "value": &value.0,
}))
.unwrap_or_default();
```

### 2. Update doc comments in `config.rs`

**File: `crates/barnum_config/src/config.rs`**

Step.finally_hook doc comment (line 110-111) — before:
```rust
/// **stdin:** The task's original `value` payload as JSON (same as what
/// the pre hook received — just the value, not the full task wrapper).
```

After:
```rust
/// **stdin:** JSON object: `{"kind": "<step name>", "value": <payload>}`.
/// Same envelope format as command action scripts.
```

FinallyHook doc comment (line 177) — before:
```rust
/// **stdin:** The task's original value payload as JSON.
```

After:
```rust
/// **stdin:** JSON object: `{"kind": "<step name>", "value": <payload>}`.
```

### 3. Regenerate schema artifacts

Run `cargo run -p barnum_cli --bin build_schemas` to regenerate:
- `libs/barnum/barnum-config-schema.json`
- `libs/barnum/barnum-config-schema.zod.ts`
- `libs/barnum/barnum-cli-schema.zod.ts`

### 4. Update demo finally hook script

**File: `crates/barnum_cli/demos/hooks/finally-hook.sh`**

Before:
```bash
item=$(echo "$input" | jq -r '.item')
```

After:
```bash
item=$(echo "$input" | jq -r '.value.item')
```

### 5. Update integration tests

**File: `crates/barnum_config/tests/finally_retry_bugs.rs`**

Multiple test functions define inline finally hook scripts. Each one that reads from stdin needs to account for the envelope format. Most test finally hooks don't parse stdin (they just echo `[]`), but any that extract values need `.value.` prefix added.

Search for all `finally` script definitions in the test file and verify each one. The pattern to look for: scripts that use `jq` or otherwise parse stdin input.

### 6. Update docs website

**Files:**
- `docs-website/docs/repertoire/hooks.md` — update stdin format description
- `docs-website/docs/repertoire/fan-out-finally.md` — update stdin format description and examples
- `docs-website/docs/reference/config-schema.md` — update stdin format description

All should document the envelope format: `{"kind": "<step name>", "value": <payload>}`.

## Behavior Change

**Breaking change for existing finally hook scripts.** Any finally hook that parses stdin (e.g., `jq '.item'`) must change to `jq '.value.item'`. Finally hooks that ignore stdin (echo `[]`) are unaffected.

After this change, every shell script barnum invokes — command actions, finally hooks — receives the same `{"kind": ..., "value": ...}` envelope on stdin.
