# GSD Pool Root Flag

## Motivation

The `agent_pool` CLI has a `--pool-root` global flag that allows overriding the default pool root directory (`/tmp/agent_pool`). The `gsd` CLI currently lacks this flag, forcing users to specify the full path as the pool name when using a custom pool root.

**Current behavior:**
```bash
# Must use full path
gsd run config.json --pool /custom/path/my-pool
```

**Desired behavior:**
```bash
# Can specify pool root separately
gsd run config.json --pool my-pool --pool-root /custom/path
```

## Current State (Before)

### `gsd_cli/src/main.rs:19-25`

```rust
#[derive(Parser)]
#[command(name = "gsd")]
#[command(about = "Get Sh*** Done - JSON-based task orchestrator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}
```

### `gsd_cli/src/main.rs:103-111`

```rust
// Resolve pool ID or path
let pool_path = pool.map_or_else(
    || {
        let temp = std::env::temp_dir().join("gsd-pool");
        std::fs::create_dir_all(&temp).ok();
        temp
    },
    |p| agent_pool::resolve_pool(&agent_pool::default_pool_root(), &p),
);
```

## Proposed State (After)

### `gsd_cli/src/main.rs` - Add global flag

```rust
#[derive(Parser)]
#[command(name = "gsd")]
#[command(about = "Get Sh*** Done - JSON-based task orchestrator")]
struct Cli {
    /// Base directory for pools. Pool IDs resolve to `<pool-root>/<id>/`.
    /// Defaults to `/tmp/agent_pool` on Unix.
    #[arg(long, global = true)]
    pool_root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}
```

### `gsd_cli/src/main.rs` - Use flag in pool resolution

```rust
// Resolve pool ID or path
let pool_root = cli.pool_root.unwrap_or_else(agent_pool::default_pool_root);
let pool_path = pool.map_or_else(
    || {
        let temp = std::env::temp_dir().join("gsd-pool");
        std::fs::create_dir_all(&temp).ok();
        temp
    },
    |p| {
        // Validate pool ID doesn't contain path separators
        if p.contains('/') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Pool ID cannot contain '/'. Use --pool-root to specify the base directory.",
            ));
        }
        Ok(agent_pool::resolve_pool(&pool_root, &p))
    },
)?;
```

Note: Need to restructure the match to access `cli.pool_root` alongside `cli.command`.

## Implementation Steps

1. Add `pool_root: Option<PathBuf>` field to `Cli` struct with `#[arg(long, global = true)]`
2. Restructure `main()` to extract `pool_root` before matching on `command`
3. Pass `pool_root` to `resolve_pool()` instead of `default_pool_root()`
4. Validate that `--pool` doesn't contain forward slashes (error with helpful message)

## Testing

Manual testing:
```bash
# Verify default behavior unchanged
gsd run config.json --pool my-pool

# Verify custom pool root works
gsd run config.json --pool my-pool --pool-root /custom/path

# Verify validation rejects paths with slashes
gsd run config.json --pool /some/path/pool  # Should error with helpful message
gsd run config.json --pool some/nested/pool  # Should error with helpful message
```

## Benefits

- Matches `agent_pool` CLI interface exactly
- Cleaner syntax for custom pool roots
- Consistent UX across `gsd` and `agent_pool` CLIs
