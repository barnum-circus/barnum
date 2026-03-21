# Pin Troupe Version to Match Barnum

## Motivation

When barnum invokes troupe via a package manager (`pnpm dlx @barnum/troupe`), it fetches whatever version is latest on npm. If barnum is at v0.2.3 but troupe latest is v0.2.4 (or the user has an older cached v0.2.2), the protocol or CLI API could be incompatible. There's no version check — it silently uses the wrong version.

## Current State

### Version is embedded at build time

`crates/barnum_cli/build.rs:3-15`:
```rust
fn main() {
    let version_path = "../../libs/barnum/version.txt";
    if let Ok(version) = std::fs::read_to_string(version_path) {
        println!("cargo:rustc-env=BARNUM_VERSION={}", version.trim());
    } else {
        println!("cargo:rustc-env=BARNUM_VERSION=unknown");
    }
}
```

Consumed in `crates/barnum_cli/src/main.rs:38`:
```rust
const VERSION: &str = env!("BARNUM_VERSION");
```

### Troupe is resolved without version pinning

`crates/cli_invoker/src/lib.rs` — the `InvokableCli` trait:
```rust
pub trait InvokableCli {
    const NPM_PACKAGE: &'static str;   // "@barnum/troupe"
    const BINARY_NAME: &'static str;    // "troupe"
    const CARGO_PACKAGE: &'static str;  // "troupe_cli"
    const ENV_VAR_BINARY: &'static str; // "TROUPE"
    const ENV_VAR_COMMAND: &'static str; // "TROUPE_COMMAND"
}
```

`crates/troupe_cli/src/lib.rs:10-16`:
```rust
impl InvokableCli for TroupeCli {
    const NPM_PACKAGE: &'static str = "@barnum/troupe";
    const BINARY_NAME: &'static str = "troupe";
    const CARGO_PACKAGE: &'static str = "troupe_cli";
    const ENV_VAR_BINARY: &'static str = "TROUPE";
    const ENV_VAR_COMMAND: &'static str = "TROUPE_COMMAND";
}
```

### Resolution order in `Invoker::detect()` (`cli_invoker/src/lib.rs:104-139`)

1. `$TROUPE` env var (binary path) — **no version issue, user controls it**
2. `$TROUPE_COMMAND` env var (full command) — **no version issue, user controls it**
3. Cargo workspace `target/debug/troupe` — **no version issue, built from same workspace**
4. `node_modules/.bin/troupe` — **no version issue, already installed specific version**
5. `packageManager` field in package.json → `pnpm dlx @barnum/troupe` — **VERSION NOT PINNED**
6. Global package manager in PATH → `pnpm dlx @barnum/troupe` — **VERSION NOT PINNED**

### Where it's invoked

`crates/barnum_cli/src/main.rs:277`:
```rust
let invoker = Invoker::<TroupeCli>::detect()?;
```

The invoker is then passed to `RunnerConfig` and used in `crates/barnum_config/src/runner/submit.rs:64-76`:
```rust
let output = invoker.run([
    "submit_task",
    "--root", root.to_str().unwrap_or("."),
    "--pool", pool_id,
    "--notify", "file",
    "--timeout-secs", "86400",
    "--data", payload,
])?;
```

### How dlx commands are built (`cli_invoker/src/lib.rs:214-227`)

```rust
fn from_package_manager(pm: &str, npm_package: &str) -> Self {
    let (program, prefix_args) = match pm {
        s if s.starts_with("pnpm") => ("pnpm", vec!["dlx", npm_package]),
        s if s.starts_with("yarn") => ("yarn", vec!["dlx", npm_package]),
        s if s.starts_with("bun") => ("bun", vec!["x", npm_package]),
        _ => ("npx", vec![npm_package]),
    };
    // ...
}
```

`npm_package` is always `"@barnum/troupe"` — no version suffix.

## Proposed Change

### Add a `version` parameter to `detect()`

The caller (barnum CLI) knows its own version. Pass it to `detect()`, and when building dlx commands, append `@{version}` to the package name.

### `crates/cli_invoker/src/lib.rs`

```rust
// Before
pub fn detect() -> io::Result<Self> { ... }

// After
pub fn detect(version: Option<&str>) -> io::Result<Self> { ... }
```

The `version` parameter flows into `from_package_manager` and `try_global_package_manager`:

```rust
// Before
fn from_package_manager(pm: &str, npm_package: &str) -> Self {
    let (program, prefix_args) = match pm {
        s if s.starts_with("pnpm") => ("pnpm", vec!["dlx", npm_package]),
        // ...
    };
}

// After
fn from_package_manager(pm: &str, npm_package: &str, version: Option<&str>) -> Self {
    let versioned = match version {
        Some(v) if v != "unknown" => format!("{npm_package}@{v}"),
        _ => npm_package.to_string(),
    };
    let (program, prefix_args) = match pm {
        s if s.starts_with("pnpm") => ("pnpm", vec!["dlx", &versioned]),
        // ...
    };
}
```

Same change for `try_global_package_manager`.

### Call sites in `crates/barnum_cli/src/main.rs`

```rust
// Before (line 277)
let invoker = Invoker::<TroupeCli>::detect()?;

// After
let invoker = Invoker::<TroupeCli>::detect(Some(VERSION))?;
```

`VERSION` is already `env!("BARNUM_VERSION")` — either a real semver string or `"unknown"` in dev builds. When `"unknown"`, we skip pinning (dev builds use the cargo workspace binary anyway, so this path isn't hit).

### Summary of files changed

| File | Change |
|------|--------|
| `crates/cli_invoker/src/lib.rs` | Add `version: Option<&str>` param to `detect()`, `from_package_manager()`, `try_global_package_manager()`. Append `@version` to npm package name in dlx commands. |
| `crates/barnum_cli/src/main.rs` | Pass `Some(VERSION)` to `detect()` at both call sites (lines 277, 327). |

### Tests to update

- `from_package_manager_pnpm` — verify output is `["dlx", "@test/pkg@1.0.0"]` when version provided
- `from_package_manager_*` — same for yarn, bun, npm
- New test: version `"unknown"` doesn't append suffix
- New test: `None` version doesn't append suffix

## Open Questions

1. **Should we also verify at runtime?** After resolving the troupe binary, we could run `troupe --version` and compare. This would catch mismatches from env var / node_modules paths too. Probably overkill for now — the dlx path is the only one that's actually broken.

2. **Exact version vs semver range?** Using `@0.2.3` pins exactly. We could use `@^0.2.3` for patch compatibility, but exact pinning is safer given the protocol coupling. Recommend exact.
