# Generic CLI Invoker with Package Manager Detection

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

### New Crate: `cli_invoker`

This is a generic utility, not specific to agent_pool or gsd. It lives in its own crate and is parameterized by a zero-sized type implementing a trait.

```
┌─────────────────┐
│  main() / CLI   │  ← Invoker::<AgentPoolCli>::detect() called here
└────────┬────────┘
         │ &Invoker<AgentPoolCli>
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

### The Trait

```rust
// crates/cli_invoker/src/lib.rs

use std::ffi::OsStr;
use std::io;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::process::{Command, Output};

/// Defines how to invoke a CLI tool via different methods.
pub trait InvokableCli {
    /// npm package name, e.g., "@gsd-now/agent-pool"
    const NPM_PACKAGE: &'static str;

    /// Binary name in target/debug/, e.g., "agent_pool_cli"
    const BINARY_NAME: &'static str;

    /// Cargo package name (for error messages), e.g., "agent_pool_cli"
    const CARGO_PACKAGE: &'static str;

    /// Environment variable for explicit binary path, e.g., "AGENT_POOL"
    const ENV_VAR_BINARY: &'static str;

    /// Environment variable for explicit command, e.g., "AGENT_POOL_COMMAND"
    const ENV_VAR_COMMAND: &'static str;
}
```

### The Invoker

```rust
/// Opaque handle for invoking a CLI tool.
/// Created once at startup, passed to functions that need it.
pub struct Invoker<T: InvokableCli> {
    kind: InvokerKind,
    _marker: PhantomData<T>,
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

impl<T: InvokableCli> Invoker<T> {
    /// Detect how to invoke the CLI.
    /// Resolution order:
    /// 1. {ENV_VAR_BINARY} env var (binary path) - CI uses this
    /// 2. {ENV_VAR_COMMAND} env var (full command)
    /// 3. Local cargo workspace binary (target/debug/{BINARY_NAME}) - local dev
    /// 4. package.json packageManager field
    /// 5. Global package manager in PATH
    pub fn detect() -> Self {
        // 1. Explicit binary path (CI sets this)
        if let Ok(path) = std::env::var(T::ENV_VAR_BINARY) {
            return Self {
                kind: InvokerKind::Binary(PathBuf::from(path)),
                _marker: PhantomData,
            };
        }

        // 2. Explicit command (e.g., "pnpm dlx @gsd-now/agent-pool")
        if let Ok(cmd) = std::env::var(T::ENV_VAR_COMMAND) {
            let parts: Vec<&str> = cmd.split_whitespace().collect();
            if !parts.is_empty() {
                return Self {
                    kind: InvokerKind::PackageManager {
                        program: parts[0].to_string(),
                        prefix_args: parts[1..].iter().map(|s| s.to_string()).collect(),
                    },
                    _marker: PhantomData,
                };
            }
        }

        // 3. Check for local cargo workspace binary (local dev)
        if let Some(binary) = find_cargo_workspace_binary(T::BINARY_NAME) {
            return Self {
                kind: InvokerKind::Binary(binary),
                _marker: PhantomData,
            };
        }

        // 4. Find package.json and detect package manager
        if let Some(pkg_manager) = detect_package_manager() {
            return Self::from_package_manager(&pkg_manager);
        }

        // 5. Fallback: check for global package managers
        Self::from_global_package_manager()
    }

    /// Run the CLI with the given arguments.
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

    /// Spawn the CLI (non-blocking).
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
            vec![T::NPM_PACKAGE.to_string()]
        } else {
            vec![dlx_arg.to_string(), T::NPM_PACKAGE.to_string()]
        };

        Self {
            kind: InvokerKind::PackageManager {
                program: program.to_string(),
                prefix_args,
            },
            _marker: PhantomData,
        }
    }

    fn from_global_package_manager() -> Self {
        let (program, dlx_arg) = if is_in_path("pnpm") {
            ("pnpm", "dlx")
        } else if is_in_path("npx") {
            ("npx", "")
        } else if is_in_path("yarn") {
            ("yarn", "dlx")
        } else {
            ("npx", "")
        };

        let prefix_args = if dlx_arg.is_empty() {
            vec![T::NPM_PACKAGE.to_string()]
        } else {
            vec![dlx_arg.to_string(), T::NPM_PACKAGE.to_string()]
        };

        Self {
            kind: InvokerKind::PackageManager {
                program: program.to_string(),
                prefix_args,
            },
            _marker: PhantomData,
        }
    }
}
```

### Helper Functions

```rust
fn is_in_path(binary: &str) -> bool {
    Command::new("which")
        .arg(binary)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Check for target/debug/{binary_name} in a cargo workspace.
fn find_cargo_workspace_binary(binary_name: &str) -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;

    loop {
        let cargo_toml = dir.join("Cargo.toml");
        if cargo_toml.exists() {
            let content = std::fs::read_to_string(&cargo_toml).ok()?;
            if content.contains("[workspace]") {
                let binary = dir.join("target/debug").join(binary_name);
                if binary.exists() {
                    return Some(binary);
                }
                // Found workspace but binary not built - fall through
                return None;
            }
        }

        if !dir.pop() {
            break;
        }
    }

    None
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
            return Some("npm".to_string());
        }

        if !dir.pop() {
            break;
        }
    }

    None
}
```

### CLI Definitions

```rust
// In agent_pool crate (or wherever appropriate)

pub struct AgentPoolCli;

impl InvokableCli for AgentPoolCli {
    const NPM_PACKAGE: &'static str = "@gsd-now/agent-pool";
    const BINARY_NAME: &'static str = "agent_pool";  // NOT agent_pool_cli
    const CARGO_PACKAGE: &'static str = "agent_pool_cli";
    const ENV_VAR_BINARY: &'static str = "AGENT_POOL";
    const ENV_VAR_COMMAND: &'static str = "AGENT_POOL_COMMAND";
}

// In gsd_cli crate

pub struct GsdCli;

impl InvokableCli for GsdCli {
    const NPM_PACKAGE: &'static str = "@gsd-now/gsd";
    const BINARY_NAME: &'static str = "gsd";
    const CARGO_PACKAGE: &'static str = "gsd_cli";
    const ENV_VAR_BINARY: &'static str = "GSD";
    const ENV_VAR_COMMAND: &'static str = "GSD_COMMAND";
}
```

**Note:** The binary names match the npm bin names:
- `agent_pool` (from `crates/agent_pool_cli/Cargo.toml` `[[bin]]` section)
- `gsd` (from `crates/gsd_cli/Cargo.toml` `[[bin]]` section)

### Usage

```rust
// gsd_cli/src/main.rs

use cli_invoker::{Invoker, InvokableCli};

fn main() -> ExitCode {
    let agent_pool = Invoker::<AgentPoolCli>::detect();

    match command {
        Command::Run { config } => run_workflow(&agent_pool, &config),
        // ...
    }
}

fn run_workflow(invoker: &Invoker<AgentPoolCli>, config: &Path) -> ExitCode {
    let output = invoker.run(["submit_task", "--pool", "foo", "--data", payload])?;
    // ...
}
```

## Resolution Order

1. **`{ENV_VAR_BINARY}` env var** - explicit binary path (CI uses this with pre-built binary)
2. **`{ENV_VAR_COMMAND}` env var** - explicit command override (e.g., `pnpm dlx @gsd-now/agent-pool`)
3. **Local cargo workspace binary** - check for `target/debug/{BINARY_NAME}` in workspace root (local dev uses this)
4. **Traverse up to find `package.json`** - check `packageManager` field
5. **Global package manager in PATH** - check for `pnpm`, then `npx`, then `yarn`

### How environments use this

| Environment | Resolution Step | Notes |
|-------------|-----------------|-------|
| CI | 1 (env var) | CI downloads pre-built binary and sets env var |
| Local dev | 3 (cargo binary) | `pnpm test` builds first, invoker finds it |
| npm user | 4 or 5 | Uses their package manager via dlx |

## Package Manager Detection

The `packageManager` field in `package.json` follows the format `<name>@<version>`:

```json
{
  "packageManager": "pnpm@10.15.0"
}
```

Mapping:
- `pnpm@*` → `pnpm dlx {NPM_PACKAGE}`
- `yarn@*` → `yarn dlx {NPM_PACKAGE}`
- `bun@*` → `bun x {NPM_PACKAGE}`
- `npm@*` or missing → `npx {NPM_PACKAGE}`

## Edge Cases

1. **No package.json found** - Fall back to global package manager detection
2. **packageManager field missing** - Assume npm, use `npx`
3. **Running from subdirectory** - Traverse up until we find package.json or Cargo.toml
4. **Windows** - Use `where` instead of `which` for PATH check
5. **No package managers installed** - Last resort uses `npx` (will fail if not installed)
6. **Cargo workspace exists but binary not built** - Falls through to package manager (no auto-build)

## Testing

### Unit tests for invoker detection

1. Test with env var set - should use binary directly
2. Test with command env var set - should use that command
3. Test in cargo workspace with built binary - should use `target/debug/{BINARY_NAME}`
4. Test in cargo workspace without built binary - should fall through to package manager
5. Test with `packageManager: "pnpm@*"` - should use `pnpm dlx`
6. Test with `packageManager: "yarn@*"` - should use `yarn dlx`
7. Test with no package.json but pnpm in PATH - should use `pnpm dlx`
8. Test from subdirectory - should find parent Cargo.toml or package.json

### Integration tests

Integration tests don't need to do anything special. They run in the cargo workspace, so:
- **Local dev**: `pnpm test` (or equivalent) builds the binary first, invoker finds it via step 3
- **CI**: Sets env var to the pre-built binary, invoker uses it via step 1

No special test setup required - the invoker "just works" in both environments.

## Implementation Steps

### Step 1: Create the `cli_invoker` crate

1. Create `crates/cli_invoker/Cargo.toml` with `serde_json` dependency
2. Create `crates/cli_invoker/src/lib.rs` with:
   - `InvokableCli` trait
   - `Invoker<T>` struct
   - `InvokerKind` enum
   - Helper functions (`is_in_path`, `find_cargo_workspace_binary`, `detect_package_manager`)
3. Add to workspace `Cargo.toml`
4. Write unit tests for detection logic

### Step 2: Define CLI types in their respective crates

1. In `agent_pool` crate: define `AgentPoolCli` implementing `InvokableCli`
2. In `gsd_cli` crate: define `GsdCli` implementing `InvokableCli`
3. Add `cli_invoker` as dependency to both

### Step 3: Update gsd_cli to use the invoker

1. In `gsd_cli/src/main.rs`:
   - Create `Invoker::<AgentPoolCli>::detect()` at startup
   - Pass `&Invoker<AgentPoolCli>` to functions that spawn agent_pool
2. Update any functions that currently use `AGENT_POOL` env var directly

### Step 4: Update CI

1. Ensure CI sets `AGENT_POOL` env var to the pre-built binary path
2. Verify tests pass with the new detection logic

### Step 5: Clean up

1. Remove any redundant env var handling from call sites
2. Remove old binary path threading if any exists
3. Update documentation

## Benefits

- **Generic** - works for any CLI tool, not coupled to agent_pool or gsd
- **Zero config** for npm/pnpm users
- **Clean architecture** - detection happens once, business logic stays clean
- **Just works** with `pnpm add @gsd-now/agent-pool` or `pnpm dlx @gsd-now/gsd`
- **Backwards compatible** - existing env vars still work
- **Respects project settings** - uses the project's configured package manager
