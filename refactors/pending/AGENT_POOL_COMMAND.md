# Generic CLI Invoker with Package Manager Detection

## Motivation

Currently, `AGENT_POOL` must point to a binary path. This creates friction for npm/pnpm users who want to:

```bash
pnpm add @gsd-now/agent-pool
# or
pnpm dlx @gsd-now/gsd run ...
```

And have it "just work" without setting environment variables.

## Resolution Order

1. **`AGENT_POOL` env var** - explicit binary path (CI uses this)
2. **`AGENT_POOL_COMMAND` env var** - explicit command (e.g., `pnpm dlx @gsd-now/agent-pool`)
3. **Cargo workspace binary** - `target/debug/agent_pool` (local dev)
4. **`node_modules/.bin/agent_pool`** - already installed via npm/pnpm
5. **`packageManager` field in package.json** - use appropriate dlx command
6. **Global package manager in PATH** - check for pnpm, npx, yarn

### How environments use this

| Environment | Resolution Step | Notes |
|-------------|-----------------|-------|
| CI | 1 (env var) | CI downloads pre-built binary and sets env var |
| Local Rust dev | 3 (cargo binary) | `cargo build` first, invoker finds it |
| npm user (installed) | 4 (node_modules) | `pnpm add @gsd-now/agent-pool` |
| npm user (not installed) | 5 or 6 | Uses dlx with detected package manager |

---

## Current State (Before)

### `gsd_config/src/runner.rs:70-71`

```rust
pub struct RunnerConfig<'a> {
    // ...
    /// Optional path to the `agent_pool` binary. If not specified, uses `AGENT_POOL` env var or PATH.
    pub agent_pool_binary: Option<&'a Path>,
}
```

### `gsd_config/src/runner.rs:1016-1025`

```rust
/// Resolve the `agent_pool` binary path.
fn resolve_agent_pool_binary() -> std::path::PathBuf {
    // 1. Environment variable override (AGENT_POOL is the standard name)
    if let Ok(path) = std::env::var("AGENT_POOL") {
        return std::path::PathBuf::from(path);
    }

    // 2. Default: assume it's in PATH
    std::path::PathBuf::from("agent_pool")
}
```

### `gsd_config/src/runner.rs:951-998`

```rust
fn submit_via_cli(
    pool_path: &Path,
    payload: &str,
    agent_pool_binary: Option<&Path>,
) -> io::Result<Response> {
    let binary = agent_pool_binary.map_or_else(resolve_agent_pool_binary, Path::to_path_buf);
    // ...
    let output = Command::new(&binary)
        .arg("submit_task")
        // ...
        .output()?;
    // ...
}
```

### `gsd_cli/src/main.rs:108-114`

```rust
let runner_config = RunnerConfig {
    agent_pool_root: &pool_path,
    config_base_path: &config_dir,
    wake_script: wake.as_deref(),
    initial_tasks,
    agent_pool_binary: None, // Use AGENT_POOL env var or PATH
};
```

---

## Proposed State (After)

### New crate: `crates/cli_invoker/src/lib.rs`

```rust
use std::ffi::OsStr;
use std::io;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::process::{Command, Output};

/// Defines how to invoke a CLI tool.
pub trait InvokableCli {
    /// npm package name, e.g., "@gsd-now/agent-pool"
    const NPM_PACKAGE: &'static str;
    /// Binary name, e.g., "agent_pool"
    const BINARY_NAME: &'static str;
    /// Cargo package name for error messages, e.g., "agent_pool_cli"
    /// Used in hints like "cargo build -p agent_pool_cli"
    const CARGO_PACKAGE: &'static str;
    /// Env var for explicit binary path, e.g., "AGENT_POOL"
    const ENV_VAR_BINARY: &'static str;
    /// Env var for explicit command, e.g., "AGENT_POOL_COMMAND"
    const ENV_VAR_COMMAND: &'static str;
}

pub struct Invoker<T: InvokableCli> {
    kind: InvokerKind,
    _marker: PhantomData<T>,
}

enum InvokerKind {
    Binary(PathBuf),
    PackageManager { program: String, prefix_args: Vec<String> },
}

impl<T: InvokableCli> Invoker<T> {
    pub fn detect() -> io::Result<Self> {
        // 1. AGENT_POOL env var
        if let Ok(path) = std::env::var(T::ENV_VAR_BINARY) {
            return Ok(Self::binary(PathBuf::from(path)));
        }

        // 2. AGENT_POOL_COMMAND env var
        if let Ok(cmd) = std::env::var(T::ENV_VAR_COMMAND) {
            if let Some(invoker) = Self::from_command_string(&cmd) {
                return Ok(invoker);
            }
        }

        // 3. Cargo workspace binary (local dev)
        if let Some(binary) = find_cargo_workspace_binary(T::BINARY_NAME) {
            return Ok(Self::binary(binary));
        }

        // 4. node_modules/.bin (already installed)
        if let Some(binary) = find_node_modules_binary(T::BINARY_NAME) {
            return Ok(Self::binary(binary));
        }

        // 5. packageManager field in package.json
        if let Some(pm) = find_package_manager_field() {
            return Ok(Self::from_package_manager(&pm, T::NPM_PACKAGE));
        }

        // 6. Global package manager in PATH
        if let Some(invoker) = Self::try_global_package_manager(T::NPM_PACKAGE) {
            return Ok(invoker);
        }

        // Nothing found - return helpful error
        Err(Self::not_found_error())
    }

    fn not_found_error() -> io::Error {
        let msg = format!(
            r#"Could not find '{binary}'. Looked in:

  1. ${env_var} environment variable (not set)
  2. ${env_var_cmd} environment variable (not set)
  3. Cargo workspace target/debug/{binary} (not found)
  4. node_modules/.bin/{binary} (not found)
  5. package.json packageManager field (not found)
  6. Global package managers pnpm/npx/yarn (not in PATH)

To fix this, either:
  - Run: cargo build -p {cargo_package}
  - Or install the package: pnpm add {npm_package}
  - Or install globally: pnpm add -g {npm_package}
  - Or set the environment variable: export {env_var}=/path/to/{binary}
  - Or install a package manager (pnpm, npm, or yarn) and run via:
      pnpm dlx {npm_package} <command>
"#,
            cargo_package = T::CARGO_PACKAGE,
            binary = T::BINARY_NAME,
            env_var = T::ENV_VAR_BINARY,
            env_var_cmd = T::ENV_VAR_COMMAND,
            npm_package = T::NPM_PACKAGE,
        );
        io::Error::new(io::ErrorKind::NotFound, msg)
    }

    pub fn run<I, S>(&self, args: I) -> io::Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        match &self.kind {
            InvokerKind::Binary(path) => Command::new(path).args(args).output(),
            InvokerKind::PackageManager { program, prefix_args } => {
                Command::new(program).args(prefix_args).args(args).output()
            }
        }
    }

    pub fn spawn<I, S>(&self, args: I) -> io::Result<std::process::Child>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        match &self.kind {
            InvokerKind::Binary(path) => Command::new(path).args(args).spawn(),
            InvokerKind::PackageManager { program, prefix_args } => {
                Command::new(program).args(prefix_args).args(args).spawn()
            }
        }
    }

    fn binary(path: PathBuf) -> Self {
        Self { kind: InvokerKind::Binary(path), _marker: PhantomData }
    }

    fn from_command_string(cmd: &str) -> Option<Self> {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }
        Some(Self {
            kind: InvokerKind::PackageManager {
                program: parts[0].to_string(),
                prefix_args: parts[1..].iter().map(|s| s.to_string()).collect(),
            },
            _marker: PhantomData,
        })
    }

    fn from_package_manager(pm: &str, npm_package: &str) -> Self {
        let (program, prefix_args) = match pm {
            s if s.starts_with("pnpm") => ("pnpm", vec!["dlx", npm_package]),
            s if s.starts_with("yarn") => ("yarn", vec!["dlx", npm_package]),
            s if s.starts_with("bun") => ("bun", vec!["x", npm_package]),
            _ => ("npx", vec![npm_package]),
        };
        Self {
            kind: InvokerKind::PackageManager {
                program: program.to_string(),
                prefix_args: prefix_args.into_iter().map(String::from).collect(),
            },
            _marker: PhantomData,
        }
    }

    fn try_global_package_manager(npm_package: &str) -> Option<Self> {
        let (program, prefix_args) = if is_in_path("pnpm") {
            ("pnpm", vec!["dlx", npm_package])
        } else if is_in_path("npx") {
            ("npx", vec![npm_package])
        } else if is_in_path("yarn") {
            ("yarn", vec!["dlx", npm_package])
        } else {
            return None; // no package manager found
        };
        Some(Self {
            kind: InvokerKind::PackageManager {
                program: program.to_string(),
                prefix_args: prefix_args.into_iter().map(String::from).collect(),
            },
            _marker: PhantomData,
        })
    }
}

fn is_in_path(binary: &str) -> bool {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    Command::new(cmd).arg(binary).output()
        .map(|o| o.status.success()).unwrap_or(false)
}

/// Traverse up from CWD looking for Cargo.toml with [workspace], check target/debug/{binary}
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
                return None; // found workspace but no binary
            }
        }
        if !dir.pop() { break; }
    }
    None
}

/// Traverse up from CWD looking for node_modules/.bin/{binary}
fn find_node_modules_binary(binary_name: &str) -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let binary = dir.join("node_modules/.bin").join(binary_name);
        if binary.exists() {
            return Some(binary);
        }
        if !dir.pop() { break; }
    }
    None
}

/// Traverse up from CWD looking for package.json with packageManager field
fn find_package_manager_field() -> Option<String> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let pkg_json = dir.join("package.json");
        if pkg_json.exists() {
            let content = std::fs::read_to_string(&pkg_json).ok()?;
            let json: serde_json::Value = serde_json::from_str(&content).ok()?;
            if let Some(pm) = json.get("packageManager").and_then(|v| v.as_str()) {
                return Some(pm.to_string());
            }
            return None; // found package.json but no packageManager field
        }
        if !dir.pop() { break; }
    }
    None
}
```

### CLI type definitions

```rust
// crates/agent_pool/src/lib.rs (or separate module)

pub struct AgentPoolCli;

impl cli_invoker::InvokableCli for AgentPoolCli {
    const NPM_PACKAGE: &'static str = "@gsd-now/agent-pool";
    const BINARY_NAME: &'static str = "agent_pool";
    const CARGO_PACKAGE: &'static str = "agent_pool_cli";
    const ENV_VAR_BINARY: &'static str = "AGENT_POOL";
    const ENV_VAR_COMMAND: &'static str = "AGENT_POOL_COMMAND";
}

// crates/gsd_cli/src/main.rs (or separate module)

pub struct GsdCli;

impl cli_invoker::InvokableCli for GsdCli {
    const NPM_PACKAGE: &'static str = "@gsd-now/gsd";
    const BINARY_NAME: &'static str = "gsd";
    const CARGO_PACKAGE: &'static str = "gsd_cli";
    const ENV_VAR_BINARY: &'static str = "GSD";
    const ENV_VAR_COMMAND: &'static str = "GSD_COMMAND";
}
```

### `gsd_config/src/runner.rs` (after)

```rust
// REMOVE: agent_pool_binary field from RunnerConfig
pub struct RunnerConfig<'a> {
    pub agent_pool_root: &'a Path,
    pub config_base_path: &'a Path,
    pub wake_script: Option<&'a str>,
    pub initial_tasks: Vec<Task>,
    pub invoker: &'a Invoker<AgentPoolCli>,  // NEW: replaces agent_pool_binary
}

// REMOVE: resolve_agent_pool_binary function entirely (lines 1016-1025)

// CHANGE: submit_via_cli signature and implementation
fn submit_via_cli(
    pool_path: &Path,
    payload: &str,
    invoker: &Invoker<AgentPoolCli>,  // CHANGED from agent_pool_binary: Option<&Path>
) -> io::Result<Response> {
    // REMOVE: let binary = agent_pool_binary.map_or_else(...)

    let pool_root = pool_path.parent().ok_or_else(|| /* ... */)?;
    let pool_id = pool_path.file_name().and_then(|s| s.to_str()).ok_or_else(|| /* ... */)?;

    let output = invoker.run([
        "submit_task",
        "--pool-root", pool_root.to_str().unwrap(),
        "--pool", pool_id,
        "--notify", "file",
        "--timeout-secs", "86400",
        "--data", payload,
    ])?;

    // ... rest unchanged
}
```

### `gsd_cli/src/main.rs` (after)

```rust
use cli_invoker::Invoker;
use agent_pool::AgentPoolCli;

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Run { config, initial, pool, wake, log_file } => {
            init_tracing(log_file.as_ref())?;

            // NEW: Create invoker once at entry point (returns helpful error if not found)
            let invoker = Invoker::<AgentPoolCli>::detect()?;

            let (cfg, config_dir) = parse_config(&config)?;
            // ...

            let runner_config = RunnerConfig {
                agent_pool_root: &pool_path,
                config_base_path: &config_dir,
                wake_script: wake.as_deref(),
                initial_tasks,
                invoker: &invoker,  // NEW: pass invoker instead of binary path
            };

            run(&cfg, &schemas, runner_config)?;
        }
        // ...
    }
}
```

---

## Implementation Steps

**Workflow:** Each step is a branch with atomic commits. CI can break during development. Before merging, restructure into clean atomic commits where CI passes on each. Merge to master without squashing.

### Step 1: Create the `cli_invoker` crate

Branch: `cli-invoker-crate`

1. Create `crates/cli_invoker/Cargo.toml`:
   ```toml
   [package]
   name = "cli_invoker"
   version = "0.1.0"
   edition = "2024"

   [dependencies]
   serde_json = "1"
   ```
2. Create `crates/cli_invoker/src/lib.rs` with trait, struct, and helper functions
3. Add `"crates/cli_invoker"` to workspace `Cargo.toml` members
4. Write unit tests for each resolution step

**Merge criteria:** Crate compiles, tests pass, CI green.

### Step 2: Define CLI types and integrate

Branch: `cli-invoker-types`

1. Add `cli_invoker` dependency to `agent_pool` and `gsd_config`
2. Define `AgentPoolCli` in `agent_pool` crate
3. Update `RunnerConfig` to take `&Invoker<AgentPoolCli>` instead of `Option<&Path>`
4. Update `submit_via_cli` to use the invoker
5. Delete `resolve_agent_pool_binary` function
6. Update `gsd_cli/src/main.rs` to create invoker and pass it

**Merge criteria:** gsd_cli uses invoker throughout, old code removed, CI green.

---

## Edge Cases

1. **No node_modules** - Fall through to packageManager field or global detection
2. **No package.json** - Fall through to global package manager detection
3. **packageManager field missing** - Fall through to global detection
4. **Running from subdirectory** - Traverse up for node_modules, package.json, Cargo.toml
5. **Windows** - Uses `where` instead of `which` for PATH check (handled via `cfg!(windows)`)
6. **Cargo workspace but binary not built** - Falls through to node_modules/package manager
7. **Multiple node_modules** - First one found traversing up wins

---

## Testing

### Unit tests for invoker detection

1. With `AGENT_POOL` env var set → uses that binary
2. With `AGENT_POOL_COMMAND` env var set → parses and uses that command
3. In cargo workspace with `target/debug/agent_pool` → uses that binary
4. In cargo workspace without binary → falls through
5. With `node_modules/.bin/agent_pool` → uses that binary
6. With `packageManager: "pnpm@10"` → uses `pnpm dlx`
7. With `packageManager: "yarn@4"` → uses `yarn dlx`
8. With pnpm in PATH but no package.json → uses `pnpm dlx`
9. From subdirectory → traverses up correctly

### Integration tests

No special setup needed:
- **Local dev**: binary found in `target/debug/` (step 3)
- **CI**: `AGENT_POOL` env var set to pre-built binary (step 1)

---

## Benefits

- **Zero config** for npm/pnpm users who installed the package
- **Clean architecture** - detection happens once, business logic stays clean
- **Works everywhere** - dev, CI, npm installed, npm dlx
- **Backwards compatible** - `AGENT_POOL` env var still works
- **Respects project settings** - uses installed binary before dlx
