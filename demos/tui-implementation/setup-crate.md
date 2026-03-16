# SetupCrate

Create the `barnum_tui` crate skeleton — Cargo.toml, minimal main.rs with CLI arg parsing, and add it to the workspace.

## Input

```json
{"workspace_root": "/path/to/barnum-worktree"}
```

The `workspace_root` is the root of a barnum git worktree where you'll write all files.

## Context

You are implementing a ratatui-based terminal dashboard for barnum workflows. This is the first step — creating the crate structure.

Read the workspace `Cargo.toml` at `{workspace_root}/Cargo.toml` to understand the existing workspace members and dependencies.

## Instructions

### 1. Add workspace dependencies

Edit `{workspace_root}/Cargo.toml` to add these to `[workspace.dependencies]`:

```toml
ratatui = "0.29"
crossterm = "0.28"
```

Also add `"crates/barnum_tui"` to the `[workspace] members` array.

### 2. Create `crates/barnum_tui/Cargo.toml`

```toml
[package]
name = "barnum_tui"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "barnum-tui"
path = "src/main.rs"

[dependencies]
barnum_config = { path = "../barnum_config" }
barnum_state = { path = "../barnum_state" }
barnum_types = { path = "../barnum_types" }
ratatui = { workspace = true }
crossterm = { workspace = true }
notify = { workspace = true }
serde = { workspace = true }
serde_json = { workspace = true }
clap = { workspace = true }
anyhow = "1"
```

### 3. Create `crates/barnum_tui/src/main.rs`

A minimal binary that parses CLI args:

```rust
use clap::Parser;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "barnum-tui", about = "Terminal dashboard for barnum workflows")]
struct Cli {
    #[arg(long)]
    config: PathBuf,

    #[arg(long)]
    state_log: PathBuf,

    #[arg(long, default_value_t = false)]
    replay: bool,
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    println!(
        "barnum-tui: config={}, state_log={}, replay={}",
        cli.config.display(),
        cli.state_log.display(),
        cli.replay
    );
    Ok(())
}
```

### 4. Verify

Run `cargo build -p barnum_tui` from the workspace root. It must compile with no errors.

### 5. Commit

```bash
git add crates/barnum_tui/ Cargo.toml Cargo.lock
git commit -m "feat(tui): add barnum_tui crate skeleton with CLI args"
```

## Output

Return exactly one follow-up task to proceed to SharedTypes:

```json
[{"kind": "SharedTypes", "value": {"workspace_root": "<same workspace_root>"}}]
```
