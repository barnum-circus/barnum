# Cleanup: Misplaced Code

## 1. `stop` doesn't belong in `submit/`

Location: `crates/agent_pool/src/submit/stop.rs`

The `stop` function controls daemon lifecycle, not task submission. It has nothing to do with submitting tasks. It should either:
- Move to its own `daemon/` or `lifecycle/` module
- Move to `pool.rs` alongside other pool management
- Become part of a `daemon_control` module

## 2. `resolve_pool` has dead code

Location: `crates/agent_pool/src/pool.rs:152-158`

```rust
pub fn resolve_pool(pool_root: &Path, reference: &str) -> PathBuf {
    if reference.contains('/') {  // <-- dead code
        PathBuf::from(reference)
    } else {
        id_to_path(pool_root, reference)
    }
}
```

We validate at CLI entry points that pool IDs don't contain path separators (`crates/agent_pool_cli/src/main.rs:447-452`). The slash check is now unreachable.

Options:
1. Simplify to just `pool_root.join(id)`
2. Delete the function entirely and use `pool_root.join(id)` at call sites
3. Add `debug_assert!(!id.contains('/'))` if we want to catch bugs during development

## 3. String literals with meaning should be constants

Example: The literal `"stop"` and `"ready"` are written/read in multiple places to control daemon state.

Locations:
- `crates/agent_pool/src/submit/stop.rs:44` - writes `"stop"`
- `crates/agent_pool/src/daemon/wiring.rs` - checks for `"stop"`
- `crates/agent_pool/src/daemon/` - writes `"ready"`

These magic strings should be constants (like `STATUS_FILE`, `LOCK_FILE`, etc. in `constants.rs`):
```rust
pub const STATUS_READY: &str = "ready";
pub const STATUS_STOP: &str = "stop";
```

Ideally this could be enforced via a lint rule, but at minimum document the pattern.
