//! Barnum CLI: the workflow engine for orchestrating AI agents.

use barnum_ast::flat::flatten;
use barnum_engine::WorkflowState;
use barnum_event_loop::{Scheduler, run_workflow};
use clap::{Parser, Subcommand, ValueEnum};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "barnum", about = "Barnum workflow engine")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// Log verbosity level for the `run` subcommand.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum LogLevel {
    /// No engine output. Only handler stderr is visible.
    Off,
    /// Fatal errors only.
    Error,
    /// Errors and warnings (default).
    #[default]
    Warn,
    /// High-level workflow progress: handler dispatch and completion.
    Info,
    /// Detailed engine internals: advance steps, state transitions.
    Debug,
    /// Maximum verbosity.
    Trace,
}

impl LogLevel {
    const fn to_tracing_filter(self) -> &'static str {
        match self {
            LogLevel::Off => "off",
            LogLevel::Error => "barnum=error",
            LogLevel::Warn => "barnum=warn",
            LogLevel::Info => "barnum=info",
            LogLevel::Debug => "barnum=debug",
            LogLevel::Trace => "barnum=trace",
        }
    }
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

        /// Engine log verbosity. Default: warn.
        #[arg(long, value_enum, default_value_t = LogLevel::Warn)]
        log_level: LogLevel,
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
            log_level,
        } => {
            init_tracing(log_level);
            run(&config, &executor, &worker).await
        }
    };
    if let Err(e) = result {
        #[expect(clippy::print_stderr)]
        {
            eprintln!("{e}");
        }
        std::process::exit(1);
    }
}

fn init_tracing(log_level: LogLevel) {
    let filter = EnvFilter::new(log_level.to_tracing_filter());
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(false)
        .init();
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
