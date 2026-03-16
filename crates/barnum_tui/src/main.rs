//! Terminal dashboard for barnum workflows.

mod app;
mod theme;

use clap::Parser;
use std::path::PathBuf;

/// Terminal dashboard for barnum workflows.
#[derive(Parser)]
#[command(name = "barnum-tui", about = "Terminal dashboard for barnum workflows")]
struct Cli {
    /// Path to the workflow config file (JSON/JSONC).
    #[arg(long)]
    config: PathBuf,

    /// Path to the NDJSON state log file.
    #[arg(long)]
    state_log: PathBuf,

    /// Replay mode: read log from beginning instead of tailing.
    #[arg(long, default_value_t = false)]
    replay: bool,
}

fn main() -> anyhow::Result<()> {
    let _cli = Cli::parse();
    Ok(())
}
