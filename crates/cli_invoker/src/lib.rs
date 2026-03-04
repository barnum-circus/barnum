//! Generic CLI invoker with package manager detection.
//!
//! This crate provides a way to invoke CLI tools that may be installed via:
//! - Direct binary path (environment variable)
//! - Cargo workspace (development)
//! - npm/pnpm/yarn (`node_modules` or dlx)
//!
//! # Example
//!
//! ```ignore
//! use cli_invoker::{Invoker, InvokableCli};
//!
//! struct MyCli;
//!
//! impl InvokableCli for MyCli {
//!     const NPM_PACKAGE: &'static str = "@my-org/my-cli";
//!     const BINARY_NAME: &'static str = "my_cli";
//!     const CARGO_PACKAGE: &'static str = "my_cli";
//!     const ENV_VAR_BINARY: &'static str = "MY_CLI";
//!     const ENV_VAR_COMMAND: &'static str = "MY_CLI_COMMAND";
//! }
//!
//! let invoker = Invoker::<MyCli>::detect()?;
//! let output = invoker.run(["--version"])?;
//! ```

use std::ffi::OsStr;
use std::io;
use std::marker::PhantomData;
use std::path::PathBuf;
use std::process::{Command, Output};

/// Defines how to invoke a CLI tool via different methods.
pub trait InvokableCli {
    /// npm package name, e.g., `"@gsd-now/agent-pool"`
    const NPM_PACKAGE: &'static str;

    /// Binary name in `target/debug/` and `node_modules/.bin/`, e.g., `"agent_pool"`
    const BINARY_NAME: &'static str;

    /// Cargo package name for error messages, e.g., `"agent_pool_cli"`.
    /// Used in hints like `cargo build -p agent_pool_cli`.
    const CARGO_PACKAGE: &'static str;

    /// Environment variable for explicit binary path, e.g., `"AGENT_POOL"`
    const ENV_VAR_BINARY: &'static str;

    /// Environment variable for explicit command, e.g., `"AGENT_POOL_COMMAND"`
    const ENV_VAR_COMMAND: &'static str;
}

/// Opaque handle for invoking a CLI tool.
///
/// Created once at startup via [`Invoker::detect`], then passed to functions
/// that need to invoke the CLI.
pub struct Invoker<T: InvokableCli> {
    kind: InvokerKind,
    _marker: PhantomData<T>,
}

impl<T: InvokableCli> Clone for Invoker<T> {
    fn clone(&self) -> Self {
        Self {
            kind: self.kind.clone(),
            _marker: PhantomData,
        }
    }
}

#[derive(Clone)]
enum InvokerKind {
    /// Direct binary path
    Binary(PathBuf),
    /// Package manager command (program, `prefix_args`).
    PackageManager {
        program: String,
        prefix_args: Vec<String>,
    },
}

impl<T: InvokableCli> Invoker<T> {
    /// Create an invoker with an explicit binary path.
    ///
    /// This bypasses detection and uses the specified path directly.
    /// Primarily useful for testing.
    #[must_use]
    pub const fn from_binary(path: PathBuf) -> Self {
        Self::binary(path)
    }

    /// Detect how to invoke the CLI.
    ///
    /// Resolution order:
    /// 1. `{ENV_VAR_BINARY}` env var (binary path) - CI uses this
    /// 2. `{ENV_VAR_COMMAND}` env var (full command)
    /// 3. Cargo workspace binary (`target/debug/{BINARY_NAME}`) - local dev
    /// 4. `node_modules/.bin/{BINARY_NAME}` - already installed
    /// 5. `packageManager` field in package.json - use dlx
    /// 6. Global package manager in PATH - use dlx
    ///
    /// # Errors
    ///
    /// Returns an error with a helpful message if no invocation method is found.
    pub fn detect() -> io::Result<Self> {
        // 1. Explicit binary path (CI sets this)
        if let Ok(path) = std::env::var(T::ENV_VAR_BINARY) {
            return Ok(Self::binary(PathBuf::from(path)));
        }

        // 2. Explicit command (e.g., "pnpm dlx @gsd-now/agent-pool")
        if let Ok(cmd) = std::env::var(T::ENV_VAR_COMMAND)
            && let Some(invoker) = Self::from_command_string(&cmd)
        {
            return Ok(invoker);
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

    /// Run the CLI with the given arguments.
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails to execute.
    pub fn run<I, S>(&self, args: I) -> io::Result<Output>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        match &self.kind {
            InvokerKind::Binary(path) => Command::new(path).args(args).output(),
            InvokerKind::PackageManager {
                program,
                prefix_args,
            } => Command::new(program).args(prefix_args).args(args).output(),
        }
    }

    /// Spawn the CLI (non-blocking).
    ///
    /// # Errors
    ///
    /// Returns an error if the command fails to spawn.
    pub fn spawn<I, S>(&self, args: I) -> io::Result<std::process::Child>
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        match &self.kind {
            InvokerKind::Binary(path) => Command::new(path).args(args).spawn(),
            InvokerKind::PackageManager {
                program,
                prefix_args,
            } => Command::new(program).args(prefix_args).args(args).spawn(),
        }
    }

    const fn binary(path: PathBuf) -> Self {
        Self {
            kind: InvokerKind::Binary(path),
            _marker: PhantomData,
        }
    }

    fn from_command_string(cmd: &str) -> Option<Self> {
        let parts: Vec<&str> = cmd.split_whitespace().collect();
        if parts.is_empty() {
            return None;
        }
        Some(Self {
            kind: InvokerKind::PackageManager {
                program: parts[0].to_string(),
                prefix_args: parts[1..].iter().map(|s| (*s).to_string()).collect(),
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
            return None;
        };
        Some(Self {
            kind: InvokerKind::PackageManager {
                program: program.to_string(),
                prefix_args: prefix_args.into_iter().map(String::from).collect(),
            },
            _marker: PhantomData,
        })
    }

    fn not_found_error() -> io::Error {
        let msg = format!(
            r"Could not find '{binary}'. Looked in:

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
",
            binary = T::BINARY_NAME,
            env_var = T::ENV_VAR_BINARY,
            env_var_cmd = T::ENV_VAR_COMMAND,
            cargo_package = T::CARGO_PACKAGE,
            npm_package = T::NPM_PACKAGE,
        );
        io::Error::new(io::ErrorKind::NotFound, msg)
    }
}

fn is_in_path(binary: &str) -> bool {
    let cmd = if cfg!(windows) { "where" } else { "which" };
    Command::new(cmd)
        .arg(binary)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Traverse up from CWD looking for `Cargo.toml` with `[workspace]`.
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

/// Traverse up from CWD looking for `node_modules/.bin/{binary}`.
fn find_node_modules_binary(binary_name: &str) -> Option<PathBuf> {
    let mut dir = std::env::current_dir().ok()?;
    loop {
        let binary = dir.join("node_modules").join(".bin").join(binary_name);
        if binary.exists() {
            return Some(binary);
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

/// Traverse up from CWD looking for `package.json` with `packageManager` field.
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
            // Found package.json but no packageManager field - fall through
            return None;
        }
        if !dir.pop() {
            break;
        }
    }
    None
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    struct TestCli;

    impl InvokableCli for TestCli {
        const NPM_PACKAGE: &'static str = "@test/test-cli";
        const BINARY_NAME: &'static str = "test_cli";
        const CARGO_PACKAGE: &'static str = "test_cli";
        const ENV_VAR_BINARY: &'static str = "TEST_CLI";
        const ENV_VAR_COMMAND: &'static str = "TEST_CLI_COMMAND";
    }

    #[test]
    fn from_command_string_parses_correctly() {
        let invoker = Invoker::<TestCli>::from_command_string("pnpm dlx @test/test-cli");
        assert!(invoker.is_some());
        let invoker = invoker.unwrap();
        match &invoker.kind {
            InvokerKind::PackageManager {
                program,
                prefix_args,
            } => {
                assert_eq!(program, "pnpm");
                assert_eq!(prefix_args, &["dlx", "@test/test-cli"]);
            }
            InvokerKind::Binary(_) => panic!("Expected PackageManager"),
        }
    }

    #[test]
    fn from_command_string_empty_returns_none() {
        let invoker = Invoker::<TestCli>::from_command_string("");
        assert!(invoker.is_none());
    }

    #[test]
    fn from_package_manager_pnpm() {
        let invoker = Invoker::<TestCli>::from_package_manager("pnpm@10.0.0", "@test/pkg");
        match &invoker.kind {
            InvokerKind::PackageManager {
                program,
                prefix_args,
            } => {
                assert_eq!(program, "pnpm");
                assert_eq!(prefix_args, &["dlx", "@test/pkg"]);
            }
            InvokerKind::Binary(_) => panic!("Expected PackageManager"),
        }
    }

    #[test]
    fn from_package_manager_yarn() {
        let invoker = Invoker::<TestCli>::from_package_manager("yarn@4.0.0", "@test/pkg");
        match &invoker.kind {
            InvokerKind::PackageManager {
                program,
                prefix_args,
            } => {
                assert_eq!(program, "yarn");
                assert_eq!(prefix_args, &["dlx", "@test/pkg"]);
            }
            InvokerKind::Binary(_) => panic!("Expected PackageManager"),
        }
    }

    #[test]
    fn from_package_manager_bun() {
        let invoker = Invoker::<TestCli>::from_package_manager("bun@1.0.0", "@test/pkg");
        match &invoker.kind {
            InvokerKind::PackageManager {
                program,
                prefix_args,
            } => {
                assert_eq!(program, "bun");
                assert_eq!(prefix_args, &["x", "@test/pkg"]);
            }
            InvokerKind::Binary(_) => panic!("Expected PackageManager"),
        }
    }

    #[test]
    fn from_package_manager_npm_default() {
        let invoker = Invoker::<TestCli>::from_package_manager("npm@10.0.0", "@test/pkg");
        match &invoker.kind {
            InvokerKind::PackageManager {
                program,
                prefix_args,
            } => {
                assert_eq!(program, "npx");
                assert_eq!(prefix_args, &["@test/pkg"]);
            }
            InvokerKind::Binary(_) => panic!("Expected PackageManager"),
        }
    }

    #[test]
    fn not_found_error_includes_all_info() {
        let err = Invoker::<TestCli>::not_found_error();
        let msg = err.to_string();
        assert!(msg.contains("test_cli"));
        assert!(msg.contains("TEST_CLI"));
        assert!(msg.contains("TEST_CLI_COMMAND"));
        assert!(msg.contains("@test/test-cli"));
        assert!(msg.contains("cargo build -p test_cli"));
    }
}
