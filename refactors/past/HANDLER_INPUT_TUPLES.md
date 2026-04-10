# Handler Input: Objects to Tuples

## Motivation

The engine currently constructs handler input as a JSON object `{ "payload": ..., "state": ... }`. Handler DAGs extract fields using `GetField("payload")` and `GetField("state")`. The RESUME_VS_RESTART_HANDLERS refactor changes all structured data to positional tuples. Converting handler input from objects to tuples can land independently on master, shrinking the refactor diff.

After this change, handler input is `[payload, state]` (a 2-element JSON array). Handler DAGs use `GetIndex(0)` for payload and `GetIndex(1)` for state.

## What changes

### 1. Engine: handler input construction

**File:** `crates/barnum_engine/src/lib.rs:499-503`

```rust
// Before
let handler_input = serde_json::json!({
    "payload": payload,
    "state": state,
});

// After
let handler_input = serde_json::json!([payload, state]);
```

### 2. TypeScript: RESTART_BODY_HANDLER

**File:** `libs/barnum/src/ast.ts:757,781-785`

The `EXTRACT_PAYLOAD` constant and `RESTART_BODY_HANDLER` extract payload from the handler input.

```ts
// Before
const EXTRACT_PAYLOAD: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "GetField", value: "payload" } },
};

// After
const EXTRACT_PAYLOAD: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "GetIndex", value: 0 } },
};
```

`RESTART_BODY_HANDLER` chains `EXTRACT_PAYLOAD` → `Tag("RestartBody")`. No change needed there — it uses `EXTRACT_PAYLOAD` by reference.

### 3. TypeScript: bind's readVar

**File:** `libs/barnum/src/bind.ts:55-65`

`readVar(n)` extracts `state[n]` from the handler input. Currently uses `GetField("state")`.

```ts
// Before
function readVar(n: number): Action {
  return {
    kind: "Chain",
    first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetField", value: "state" } } },
    rest: {
      kind: "Chain",
      first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetIndex", value: n } } },
      rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Resume" } } },
    },
  };
}

// After
function readVar(n: number): Action {
  return {
    kind: "Chain",
    first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetIndex", value: 1 } } },
    rest: {
      kind: "Chain",
      first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "GetIndex", value: n } } },
      rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Resume" } } },
    },
  };
}
```

`GetField("state")` → `GetIndex(1)`. The rest of the chain is unchanged.

### 4. TypeScript: bind.ts comment

**File:** `libs/barnum/src/bind.ts:47-53`

Update the doc comment on `readVar` to reflect the new structure:

```ts
// Before
 * handler with `{ payload, state }`. For bind, `state` is the full All
 * Expanded AST: Chain(GetField("state"), Chain(GetIndex(n), Tag("Resume")))

// After
 * handler with `[payload, state]`. For bind, `state` (index 1) is the full All
 * Expanded AST: Chain(GetIndex(1), Chain(GetIndex(n), Tag("Resume")))
```

### 5. Rust tests: handler DAGs and assertions

**File:** `crates/barnum_engine/src/lib.rs`

Multiple test helpers and assertions reference `GetField("payload")`, `GetField("state")`, and `json!({ "payload": ..., "state": ... })`.

**Test helper `restart_body_handler()`** (line 1393):
```rust
// Before
fn restart_body_handler() -> Action {
    chain(get_field("payload"), tag_builtin("RestartBody"))
}

// After
fn restart_body_handler() -> Action {
    chain(get_index(0), tag_builtin("RestartBody"))
}
```

**Test helper `echo_resume_handler()`** (line 1420-1427):
```rust
// Before — extracts "payload", tags "Resume"
invoke_builtin(BuiltinKind::GetField {
    value: json!("payload"),
}),

// After
invoke_builtin(BuiltinKind::GetIndex {
    value: json!(0),
}),
```

**Test `resume_with_state()`** (line 1957-1962):
```rust
// Before — handler DAG: GetField("state") -> Tag("Resume")
invoke_builtin(BuiltinKind::GetField {
    value: json!("state"),
}),

// After
invoke_builtin(BuiltinKind::GetIndex {
    value: json!(1),
}),
```

**Test assertions** — all `json!({ "payload": ..., "state": ... })` assertions change to `json!([..., ...])`:
- Line 1979: `json!({ "payload": "step_out", "state": "input" })` → `json!(["step_out", "input"])`
- Line 2032: `json!({ "payload": "mid_out", "state": "new_state" })` → `json!(["mid_out", "new_state"])`
- Line 2341: comment update
- Line 2368-2372: `json!({ "payload": "input", "state": [42, "input"] })` → `json!(["input", [42, "input"]])`

### 6. Not changed: GetField("value") in Branch

`GetField("value")` in `unwrapBranchCases()` (ast.ts:710, builtins.ts:328) extracts the `value` field from tagged variants (`{ kind: "Continue", value: ... }`). This is unrelated to handler input — it's the Branch routing mechanism. No change.

### 7. Not changed: getField as a user-facing combinator

`getField()` in `builtins.ts` is a general-purpose combinator for extracting fields from objects. It remains available for user pipelines. The change is only to handler input construction and handler DAGs that extract from handler input.

## Verification

After making these changes:
1. `pnpm run typecheck` from repo root — TypeScript changes don't affect types (handler DAGs are untyped `Action` values).
2. `cargo test` in `crates/barnum_engine` — all handler input assertions updated.
3. Run full test suite to verify end-to-end behavior.
