mod app;
mod detail;
mod event;
mod footer;
mod graph;
mod header;
mod log_watcher;
mod task_list;
mod theme;

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
