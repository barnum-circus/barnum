# CLIShim

Add a `tui` subcommand to `barnum_cli` that delegates to the `barnum-tui` binary, keeping TUI dependencies out of the main CLI.

## Input

```json
{"workspace_root": "/path/to/barnum-worktree"}
```

## Context

The `barnum-tui` binary is fully implemented. Now add a thin shim in `barnum_cli` so users can run `barnum tui --config ... --state-log ...`.

Read:
- `{workspace_root}/crates/barnum_cli/src/main.rs` — understand the existing `Command` enum and how subcommands are dispatched

## Instructions

### 1. Modify `crates/barnum_cli/src/main.rs`

Add a `Tui` variant to the `Command` enum:

```rust
/// Launch the TUI dashboard (requires barnum-tui binary)
Tui {
    /// Arguments passed through to barnum-tui
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
},
```

Add the match arm in the command dispatch:

```rust
Command::Tui { args } => {
    let status = std::process::Command::new("barnum-tui")
        .args(&args)
        .status()
        .context("Failed to run barnum-tui. Is it installed? Try: cargo install --path crates/barnum_tui")?;
    std::process::exit(status.code().unwrap_or(1));
}
```

### 2. Verify

Run `cargo build -p barnum_cli -p barnum_tui`. Both must compile.

Then test the shim:
```bash
cargo run -p barnum_cli -- tui --help
```
This should show the barnum-tui help output (assuming barnum-tui is on PATH or built).

### 3. Commit

```bash
git add crates/barnum_cli/src/main.rs
git commit -m "feat(cli): add 'barnum tui' shim subcommand"
```

## Output

This is the final step. Return an empty array:

```json
[]
```
