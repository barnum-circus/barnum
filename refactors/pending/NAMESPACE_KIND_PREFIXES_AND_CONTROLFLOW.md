# Namespace Rust Kind Prefixes

**Blocks:** UNION_DISPATCH_AST_NODES.md (ExtractPrefix requires namespaced kinds)

## Motivation

Rust builtins produce unprefixed kinds (`"Some"` instead of `"Option.Some"`), and engine test helpers construct tagged union values by hand (`json!({ "kind": "Continue", ... })`). There's no abstraction encapsulating kind string construction. The TS SDK has `tag("Some", "Option")` and wrapper functions like `Option.some` that make the namespace a single source of truth. The Rust side has no equivalent — every call site constructs the JSON inline with a raw string.

This is the root cause of the inconsistency: without a constructor function, there's nothing enforcing that kinds are namespaced. A wrapper function like `tagged_value("Some", "Option", value)` → `{ kind: "Option.Some", value }` would make the prefix impossible to forget.

This blocks UNION_DISPATCH because `ExtractPrefix` splits on `'.'` and can't work with unprefixed kinds.

## Current state

### No constructor abstraction

Every tagged union value is constructed inline:

```rust
// crates/barnum_builtins/src/lib.rs — GetIndex (lines 117-118)
Some(value) => Ok(json!({ "kind": "Some", "value": value })),
None => Ok(json!({ "kind": "None", "value": null })),

// SplitFirst (lines 131, 135), SplitLast (lines 148, 152) — same pattern

// CollectSome (line 170) — reads the kind string inline
if obj.get("kind").and_then(Value::as_str) == Some("Some") {
```

### Engine test helpers also construct tags inline

```rust
// crates/barnum_engine/src/test_helpers.rs — tag_action (line 76)
pub fn tag_action(kind: &str) -> Action {
    // Bakes the kind string directly into a Constant builtin
    // No enum name, no namespacing
}

// Called with bare variant names:
// break_restart_perform (line 154): tag_action("Break")
// restart_branch (line 170): tag_action("Continue")
```

### Engine tests construct JSON values by hand

```rust
// crates/barnum_engine/src/effects.rs — restart_branch_multiple_then_break
value: json!({"kind": "Continue", "value": "restarted"}),  // line 316
value: json!({"kind": "Break", "value": "gave_up"}),       // line 341

// crates/barnum_engine/src/advance.rs — branch_dispatches_matching_case
json!({"kind": "Ok", "value": 42})                         // line 322

// crates/barnum_engine/tests/advance/branch_matching.json
"input": { "kind": "Err", "error": "something failed" }    // line 17
```

### Engine tests use raw GetIndex without Option unwrapping

```rust
// crates/barnum_engine/src/effects.rs — bind tests
invoke_builtin(BuiltinKind::GetIndex { index: 1 }),  // lines 740, 858, 870, 945, 975
// These expect the raw value, but after the fix GetIndex returns Option-wrapped output
```

## Changes

### Part 1: Add `tagged_value` constructor to barnum_builtins

Add a function that's the single source of truth for constructing tagged union JSON values:

```rust
/// Construct a tagged union value: `{ "kind": "{enum_name}.{variant}", "value": value }`.
pub fn tagged_value(variant: &str, enum_name: &str, value: Value) -> Value {
    json!({ "kind": format!("{enum_name}.{variant}"), "value": value })
}

/// Check whether a value is a specific tagged variant.
pub fn is_variant(value: &Value, variant: &str, enum_name: &str) -> bool {
    value.get("kind").and_then(Value::as_str)
        == Some(&format!("{enum_name}.{variant}"))
}
```

Use these everywhere instead of raw `json!()`:

```rust
// GetIndex
Some(value) => Ok(tagged_value("Some", "Option", value.clone())),
None => Ok(tagged_value("None", "Option", Value::Null)),

// SplitFirst, SplitLast — same pattern

// CollectSome
if is_variant(item, "Some", "Option") {
```

### Part 2: Fix engine test helpers

**`crates/barnum_engine/src/test_helpers.rs`:**

1. **`tag_action`** — require enum_name:
```rust
pub fn tag_action(variant: &str, enum_name: &str) -> Action {
    let kind = format!("{enum_name}.{variant}");
    chain(
        parallel(vec![
            chain(
                invoke_builtin(BuiltinKind::Constant { value: json!(kind) }),
                invoke_builtin(BuiltinKind::WrapInField { field: "kind".to_string() }),
            ),
            invoke_builtin(BuiltinKind::WrapInField { field: "value".to_string() }),
        ]),
        invoke_builtin(BuiltinKind::Merge),
    )
}
```

2. **`get_index`** — unwrap the Option:
```rust
/// GetIndex that unwraps the Option. BranchNoMatch error if None.
pub fn get_index(index: usize) -> Action {
    chain(
        get_index_option(index),
        branch(vec![("Some", get_field("value"))]),
    )
}

/// Raw GetIndex that returns Option<T>.
pub fn get_index_option(index: usize) -> Action {
    invoke_builtin(BuiltinKind::GetIndex { index })
}
```

3. **Update callers of `tag_action`** — use `"LoopResult"` as the enum name for now (LoopResult → ControlFlow rename is separate and not blocking):
```rust
pub fn break_restart_perform(restart_handler_id: u16) -> Action {
    chain(tag_action("Break", "LoopResult"), restart_perform(restart_handler_id))
}

// restart_branch: tag_action("Continue", "LoopResult")
```

4. **`resume_read_var`** — no change needed, `get_index()` now auto-unwraps.

### Part 3: Fix engine tests

**Replace direct `invoke_builtin(BuiltinKind::GetIndex { ... })` with `get_index(N)`:**
- `effects.rs` lines 740, 858, 870, 945, 975

**Replace manually constructed unprefixed kind JSON:**
- `effects.rs` line 316: `"LoopResult.Continue"` (matches what TS SDK currently produces)
- `effects.rs` line 341: `"LoopResult.Break"`
- `advance.rs` line 322: `"Result.Ok"`

**Fix test fixture:**
- `tests/advance/branch_matching.json` line 17: `"Result.Err"`

**Update snapshots:** `branch_matching.json` snapshot will change. Run with `INSTA_UPDATE=1`.

**Fix builtin test assertions** in `barnum_builtins/src/lib.rs`:
- All `"Some"` → `"Option.Some"`, `"None"` → `"Option.None"` in test assertions and test inputs

## What doesn't change

- **Branch case keys stay unprefixed.** Branch cases are `"Ok"`, `"Err"`, `"Some"`, `"Continue"`, etc. The engine's `rsplit_once('.')` prefix stripping handles this — `"LoopResult.Continue"` strips to `"Continue"`, matching the `"Continue"` case.
- **LoopResult name stays for now.** LoopResult → ControlFlow is a separate rename that isn't blocking.

## Test-first approach

1. **Commit 1:** Add `#[should_panic]` tests asserting namespaced kinds from builtins.
2. **Commit 2:** Add `tagged_value` / `is_variant` constructors. Fix builtins. Remove `#[should_panic]`. Update builtin test assertions.
3. **Commit 3:** Fix engine test helpers (`tag_action` signature, `get_index` unwrapping).
4. **Commit 4:** Fix engine tests (replace direct GetIndex, manual JSON, fixture). Update snapshots.
5. **Commit 5:** Run full test suite, verify green.
