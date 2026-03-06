# Explicit Inline Format

**Status:** Not started

**Depends on:** Nothing

**Blocks:** INLINED_CONFIG (must have consistent format before introducing MaybeLinked<T>)

## Motivation

Currently, `Instructions` uses `#[serde(untagged)]` which allows bare strings:

```json
"instructions": "Do the thing"
```

This is inconsistent with the link format:

```json
"instructions": {"link": "path/to/file.md"}
```

We want all instructions to use explicit object format:

```json
"instructions": {"inline": "Do the thing"}
```

This makes the format consistent and prepares for `MaybeLinked<T>` which will handle both `{"inline": ...}` and `{"link": ...}` uniformly.

## Current vs Goal

### Current (config.rs)

```rust
/// In config files:
/// - String → inline markdown
/// - `{"link": "path"}` → link to markdown file
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Instructions {
    /// Inline markdown text.
    Inline(String),
    /// Link to a markdown file.
    Link {
        /// Path to the markdown file.
        link: String,
    },
}
```

### Goal (config.rs)

```rust
/// In config files:
/// - `{"inline": "text"}` → inline markdown
/// - `{"link": "path"}` → link to markdown file
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Instructions {
    /// Inline markdown text.
    Inline {
        /// The markdown text.
        inline: String,
    },
    /// Link to a markdown file.
    Link {
        /// Path to the markdown file.
        link: String,
    },
}
```

### Config Format Change

**Before:**
```json
{"name": "Start", "action": {"kind": "Pool", "instructions": "Do something"}, "next": []}
```

**After:**
```json
{"name": "Start", "action": {"kind": "Pool", "instructions": {"inline": "Do something"}}, "next": []}
```

## Files to Update

### Rust Code

| File | Change |
|------|--------|
| `crates/gsd_config/src/config.rs:244-252` | Change `Inline(String)` to `Inline { inline: String }` |
| `crates/gsd_config/src/config.rs:254-258` | Update `Default` impl |
| `crates/gsd_config/src/config.rs:260-268` | Update `as_inline()` method |
| `crates/gsd_config/src/config.rs:570` | Update test pattern match |
| `crates/gsd_config/src/docs.rs:16,34` | Update pattern matches |

### Tests (all use bare string format)

| File | Lines |
|------|-------|
| `crates/gsd_cli/tests/cli_integration.rs` | 51, 106-108, 155, 180-181, 198, 213, 230-231, 271 |
| `crates/gsd_cli/tests/config_subcommands.rs` | 311-312, 334, 411 |
| `crates/gsd_config/tests/concurrency.rs` | 34, 228, 388-394 |
| `crates/gsd_config/tests/linear_transitions.rs` | 26, 31, 36 |
| `crates/gsd_config/tests/invalid_transitions.rs` | 31, 36, 41 |
| `crates/gsd_config/tests/retry_behavior.rs` | 60, 65, 130, 135, 200, 267, 275, 346, 408 |
| `crates/gsd_config/tests/edge_cases.rs` | 41, 122, 181 |
| `crates/gsd_config/tests/simple_termination.rs` | 25 |
| `crates/gsd_config/tests/branching_transitions.rs` | 28, 33, 38, 43 |

### Demos

| File | Change |
|------|--------|
| `crates/gsd_cli/demos/linear/config.jsonc:14,27,40` | Wrap in `{"inline": ...}` |
| `crates/gsd_cli/demos/simple/config.jsonc:15` | Wrap in `{"inline": ...}` |
| `crates/gsd_cli/demos/branching/config.jsonc:17,30,43,56` | Wrap in `{"inline": ...}` |
| `crates/gsd_cli/demos/fan-out/config.jsonc:14,33` | Wrap in `{"inline": ...}` |
| `crates/gsd_cli/demos/refactor-workflow/config.jsonc` | Already uses `{"link": ...}` - no change |

## Implementation

1. Update `Instructions` enum in `config.rs`
2. Update pattern matches in `docs.rs`
3. Update tests in `config.rs`
4. Update all test files
5. Update all demo config files
6. Run tests to verify

## After This Refactor

Both `Instructions` and `SchemaRef` will have consistent formats:

```rust
// Both use the same pattern
pub enum Instructions {
    Inline { inline: String },
    Link { link: String },
}

pub enum SchemaRef {
    Inline(serde_json::Value),  // Note: inline schema is the JSON itself, not wrapped
    Link { link: String },
}
```

Note: `SchemaRef::Inline` keeps the value directly because inline JSON schema IS an object. We can't wrap it in `{"inline": ...}` without changing the schema format. This is fine - the key insight is that links are always `{"link": "path"}`.
