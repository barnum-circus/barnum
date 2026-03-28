//! Barnum workflow engine CLI.

use barnum_ast::flat::flatten;
use barnum_engine::WorkflowState;
use barnum_event_loop::{Scheduler, run_workflow};
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "barnum", about = "Barnum workflow engine")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Deserialize a workflow config, reserialize, and print. Used for
    /// round-trip validation.
    Check {
        /// Serialized JSON config.
        #[arg(long)]
        config: String,
    },

    /// Run a workflow to completion.
    Run {
        /// Serialized JSON config.
        #[arg(long)]
        config: String,

        /// Executor command for TypeScript (e.g., "node /path/to/tsx/cli.mjs").
        #[arg(long)]
        executor: String,

        /// Path to worker.ts.
        #[arg(long)]
        worker: String,
    },
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let result = match cli.command {
        Command::Check { config } => check(&config),
        Command::Run {
            config,
            executor,
            worker,
        } => run(&config, &executor, &worker).await,
    };
    if let Err(e) = result {
        #[expect(clippy::print_stderr)]
        {
            eprintln!("{e}");
        }
        std::process::exit(1);
    }
}

fn check(input: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config: barnum_ast::Config = serde_json::from_str(input)?;
    let output = serde_json::to_string_pretty(&config)?;
    #[expect(clippy::print_stdout)]
    {
        println!("{output}");
    }
    Ok(())
}

async fn run(input: &str, executor: &str, worker: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config: barnum_ast::Config = serde_json::from_str(input)?;
    let flat_config = flatten(config)?;
    let mut workflow_state = WorkflowState::new(flat_config);
    let mut scheduler = Scheduler::new(executor.to_owned(), worker.to_owned());

    let result = run_workflow(&mut workflow_state, &mut scheduler).await?;

    #[expect(clippy::print_stdout)]
    {
        println!("{}", serde_json::to_string_pretty(&result)?);
    }
    Ok(())
}
