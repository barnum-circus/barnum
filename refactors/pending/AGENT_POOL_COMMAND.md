# Support automatic package manager detection for agent_pool

## Motivation

Currently, `AGENT_POOL` must point to a binary path. This creates friction for npm/pnpm users who want to:

```bash
pnpm add @gsd-now/agent-pool
# or
pnpm dlx @gsd-now/gsd run ...
```

And have it "just work" without setting environment variables.

## Architecture

**Key principle:** Resolve the invocation method ONCE at program startup, then pass an opaque invoker through the call stack. Detection logic never leaks into business logic.

```
┌─────────────────┐
│  main() / CLI   │  ← AgentPoolInvoker::detect() called here
└────────┬────────┘
         │ &AgentPoolInvoker
         ▼
┌─────────────────┐
│  submit_task()  │  ← invoker.run(&["submit_task", ...])
└────────┬────────┘
         │
         ▼
┌─────────────────┐
│  other fns...   │  ← just passes invoker through, never inspects it
└─────────────────┘
```

## Implementation

### The Invoker Struct

```rust
// crates/agent_pool/src/invoker.rs

use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// Opaque handle for invoking agent_pool_cli.
/// Created once at startup, passed to functions that need it.
pub struct AgentPoolInvoker {
    kind: InvokerKind,
}

enum InvokerKind {
    /// Direct binary path
    Binary(PathBuf),
    /// Package manager: (program, prefix_args)
    /// e.g., ("pnpm", ["dlx", "@gsd-now/agent-pool"])
    PackageManager {
        program: String,
        prefix_args: Vec<String>,
    },
}

impl AgentPoolInvoker {
    /// Detect how to invoke agent_pool_cli.
    /// Resolution order:
    /// 1. AGENT_POOL env var (binary path)
    /// 2. AGENT_POOL_COMMAND env var (full command)
    /// 3. package.json packageManager field
    /// 4. Global package manager in PATH
    pub fn detect() -> Self {
        // 1. Explicit binary path
        if let Ok(path) = std::env::var("AGENT_POOL") {
            return Self {
                kind: InvokerKind::Binary(PathBuf::from(path)),
            };
        }

        // 2. Explicit command (e.g., "pnpm dlx @gsd-now/agent-pool")
        if let Ok(cmd) = std::env::var("AGENT_POOL_COMMAND") {
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            if !parts.is_empty() {
                return Self {
                    kind: InvokerKind::PackageManager {
                        program: parts[0].to_string(),
                        prefix_args: parts[1..].iter().map(|s| s.to_string()).collect(),
                    },
                };
            }
        }

        // 3. Find package.json and detect package manager
        if let Some(pkg_manager) = detect_package_manager() {
            return Self::from_package_manager(&pkg_manager);
        }

        // 4. Fallback: check for global package managers
        Self::from_global_package_manager()
    }

    /// Run agent_pool_cli with the given arguments.
    pub fn run<I, S>(&self, args: I) -> io::Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        match &self.kind {
            InvokerKind::Binary(path) => Command::new(path).args(args).output(),
            InvokerKind::PackageManager { program, prefix_args } => Command::new(program)
                .args(prefix_args)
                .args(args)
                .output(),
        }
    }

    /// Spawn agent_pool_cli (non-blocking).
    pub fn spawn<I, S>(&self, args: I) -> io::Result<std::process::Child>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        match &self.kind {
            InvokerKind::Binary(path) => Command::new(path).args(args).spawn(),
            InvokerKind::PackageManager { program, prefix_args } => Command::new(program)
                .args(prefix_args)
                .args(args)
                .spawn(),
        }
    }

    fn from_package_manager(pm: &str) -> Self {
        let (program, dlx_arg) = match pm {
            s if s.starts_with("pnpm") => ("pnpm", "dlx"),
            s if s.starts_with("yarn") => ("yarn", "dlx"),
            s if s.starts_with("bun") => ("bun", "x"),
            _ => ("npx", ""),
        };

        let prefix_args = if dlx_arg.is_empty() {
            vec!["@gsd-now/agent-pool".to_string()]
        } else {
            vec![dlx_arg.to_string(), "@gsd-now/agent-pool".to_string()]
        };

        Self {
            kind: InvokerKind::PackageManager {
                program: program.to_string(),
                prefix_args,
            },
        }
    }

    fn from_global_package_manager() -> Self {
        let (program, prefix_args) = if is_in_path("pnpm") {
            ("pnpm", vec!["dlx", "@gsd-now/agent-pool"])
        } else if is_in_path("npx") {
            ("npx", vec!["@gsd-now/agent-pool"])
        } else if is_in_path("yarn") {
            ("yarn", vec!["dlx", "@gsd-now/agent-pool"])
        } else {
            // Last resort
            ("npx", vec!["@gsd-now/agent-pool"])
        };

        Self {
            kind: InvokerKind::PackageManager {
                program: program.to_string(),
                prefix_args: prefix_args.into_iter().map(String::from).collect(),
            },
        }
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

### Usage at Entry Points

```rust
// gsd_cli/src/main.rs

fn main() -> ExitCode {
    let invoker = AgentPoolInvoker::detect();

    // ... parse args ...

    match command {
        Command::Run { config } => run_workflow(&invoker, &config),
        // ...
    }
}

fn run_workflow(invoker: &AgentPoolInvoker, config: &Path) -> ExitCode {
    // invoker passed to anything that needs to spawn agents
    let pool = start_pool(invoker, &pool_root)?;
    // ...
}
```

### Call Sites Just Use `.run()`

```rust
// Before (leaky - knows about binary paths)
fn submit_via_cli(
    pool_path: &Path,
    payload: &str,
    agent_pool_binary: Option<&Path>,  // ← leaky
) -> io::Result<Response> {
    let binary = agent_pool_binary.ok_or_else(|| ...)?;
    Command::new(binary)
        .args(["submit_task", ...])
        .output()
}

// After (clean - just uses invoker)
fn submit_via_cli(
    invoker: &AgentPoolInvoker,  // ← opaque
    pool_path: &Path,
    payload: &str,
) -> io::Result<Response> {
    let output = invoker.run([
        "submit_task",
        "--pool", pool_path.to_str().unwrap(),
        "--notify", "file",
        "--data", payload,
    ])?;
    // ...
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

## Resolution Order

1. **`AGENT_POOL` env var** - explicit binary path override
2. **`AGENT_POOL_COMMAND` env var** - explicit command override (e.g., `pnpm dlx @gsd-now/agent-pool`)
3. **Traverse up to find `package.json`** - check `packageManager` field
4. **Global package manager in PATH** - check for `pnpm`, then `npx`, then `yarn`

## Edge Cases

1. **No package.json found** - Fall back to global package manager detection
2. **packageManager field missing** - Assume npm, use `npx`
3. **Running from subdirectory** - Traverse up until we find package.json
4. **Windows** - Use `where` instead of `which` for PATH check
5. **No package managers installed** - Last resort uses `npx` (will fail if not installed)

## Testing

1. Test with `AGENT_POOL` set - should use binary directly
2. Test with `AGENT_POOL_COMMAND` set - should use that command
3. Test with `packageManager: "pnpm@*"` - should use `pnpm dlx`
4. Test with `packageManager: "yarn@*"` - should use `yarn dlx`
5. Test with no package.json but pnpm in PATH - should use `pnpm dlx`
6. Test with no package.json but only npx in PATH - should use `npx`
7. Test from subdirectory - should find parent package.json

## Benefits

- **Zero config** for npm/pnpm users
- **Clean architecture** - detection happens once, business logic stays clean
- **Just works** with `pnpm add @gsd-now/agent-pool` or `pnpm dlx @gsd-now/gsd`
- **Backwards compatible** - existing `AGENT_POOL` env var still works
- **Respects project settings** - uses the project's configured package manager
