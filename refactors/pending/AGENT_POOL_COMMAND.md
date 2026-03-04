# Support automatic package manager detection for agent_pool

## Motivation

Currently, `AGENT_POOL` must point to a binary path. This creates friction for npm/pnpm users who want to:

```bash
pnpm add @gsd-now/agent-pool
# or
pnpm dlx @gsd-now/gsd run ...
```

And have it "just work" without setting environment variables.

## Proposed Solution

Automatically detect the package manager and use `npx` or `pnpm dlx` when the binary isn't found.

### Resolution Order

1. **`AGENT_POOL` env var** - explicit binary path override
2. **`agent_pool` in PATH** - global install or PATH configured
3. **`./node_modules/.bin/agent_pool`** - local npm/pnpm install
4. **Traverse up to find `package.json`** - check `packageManager` field, use appropriate dlx command

### Implementation

```rust
use std::path::{Path, PathBuf};
use std::process::Command;

/// How to invoke the agent_pool binary
enum AgentPoolInvocation {
    /// Direct binary path
    Binary(PathBuf),
    /// Package manager command: (program, args_before_subcommand)
    /// e.g., ("pnpm", ["dlx", "@gsd-now/agent-pool"])
    PackageManager { program: String, prefix_args: Vec<String> },
}

fn resolve_agent_pool_invocation() -> AgentPoolInvocation {
    // 1. Explicit env var
    if let Ok(path) = std::env::var("AGENT_POOL") {
        return AgentPoolInvocation::Binary(PathBuf::from(path));
    }

    // 2. Check PATH
    if is_in_path("agent_pool") {
        return AgentPoolInvocation::Binary(PathBuf::from("agent_pool"));
    }

    // 3. Check local node_modules
    let local_bin = Path::new("./node_modules/.bin/agent_pool");
    if local_bin.exists() {
        return AgentPoolInvocation::Binary(
            local_bin.canonicalize().unwrap_or_else(|_| local_bin.to_path_buf())
        );
    }

    // 4. Find package.json and detect package manager
    if let Some(pkg_manager) = detect_package_manager() {
        let (program, dlx_arg) = match pkg_manager.as_str() {
            pm if pm.starts_with("pnpm") => ("pnpm", "dlx"),
            pm if pm.starts_with("yarn") => ("yarn", "dlx"),
            pm if pm.starts_with("bun") => ("bun", "x"),
            _ => ("npx", ""), // npm or unknown - use npx
        };

        let prefix_args = if dlx_arg.is_empty() {
            vec!["@gsd-now/agent-pool".to_string()]
        } else {
            vec![dlx_arg.to_string(), "@gsd-now/agent-pool".to_string()]
        };

        return AgentPoolInvocation::PackageManager {
            program: program.to_string(),
            prefix_args,
        };
    }

    // 5. Fallback: try npx
    AgentPoolInvocation::PackageManager {
        program: "npx".to_string(),
        prefix_args: vec!["@gsd-now/agent-pool".to_string()],
    }
}

fn is_in_path(binary: &str) -> bool {
    Command::new("which")
        .arg(binary)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Traverse up from CWD to find package.json and read packageManager field
fn detect_package_manager() -> Option<String> {
    let mut dir = std::env::current_dir().ok()?;

    loop {
        let pkg_json = dir.join("package.json");
        if pkg_json.exists() {
            let content = std::fs::read_to_string(&pkg_json).ok()?;
            let json: serde_json::Value = serde_json::from_str(&content).ok()?;
            if let Some(pm) = json.get("packageManager").and_then(|v| v.as_str()) {
                return Some(pm.to_string());
            }
            // Found package.json but no packageManager - assume npm
            return Some("npm".to_string());
        }

        if !dir.pop() {
            break;
        }
    }

    None
}
```

### Invocation

Update `submit_via_cli` to handle both cases:

```rust
fn submit_via_cli(
    pool_path: &Path,
    payload: &str,
    agent_pool_binary: Option<&Path>,
) -> io::Result<Response> {
    let invocation = agent_pool_binary
        .map(|p| AgentPoolInvocation::Binary(p.to_path_buf()))
        .unwrap_or_else(resolve_agent_pool_invocation);

    let cli_args = [
        "submit_task",
        "--pool-root", pool_root.to_str().unwrap(),
        "--pool", pool_id,
        "--notify", "file",
        "--timeout-secs", "86400",
        "--data", payload,
    ];

    let output = match invocation {
        AgentPoolInvocation::Binary(binary) => {
            Command::new(&binary)
                .args(&cli_args)
                .output()
        }
        AgentPoolInvocation::PackageManager { program, prefix_args } => {
            Command::new(&program)
                .args(&prefix_args)
                .args(&cli_args)
                .output()
        }
    }.map_err(|e| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("Failed to run agent_pool: {e}"),
        )
    })?;

    // ... rest unchanged
}
```

## Package Manager Detection

The `packageManager` field in `package.json` follows the format `<name>@<version>`:

```json
{
  "packageManager": "pnpm@10.15.0"
}
```

Mapping:
- `pnpm@*` → `pnpm dlx @gsd-now/agent-pool`
- `yarn@*` → `yarn dlx @gsd-now/agent-pool`
- `bun@*` → `bun x @gsd-now/agent-pool`
- `npm@*` or missing → `npx @gsd-now/agent-pool`

## Edge Cases

1. **No package.json found** - Fall back to `npx` (most common)
2. **packageManager field missing** - Assume npm, use `npx`
3. **Running from subdirectory** - Traverse up until we find package.json
4. **Windows** - Use `where` instead of `which` for PATH check

## Testing

1. Test with `AGENT_POOL` set - should use that directly
2. Test with binary in PATH - should use it
3. Test with local `node_modules/.bin/agent_pool` - should use it
4. Test with `packageManager: "pnpm@*"` - should use `pnpm dlx`
5. Test with no package.json - should fallback to `npx`
6. Test from subdirectory - should find parent package.json

## Benefits

- **Zero config** for npm/pnpm users
- **Just works** with `pnpm add @gsd-now/agent-pool` or `pnpm dlx @gsd-now/gsd`
- **Backwards compatible** - existing `AGENT_POOL` env var still works
- **Respects project settings** - uses the project's configured package manager
