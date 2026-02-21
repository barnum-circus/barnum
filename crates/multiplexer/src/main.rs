//! CLI for the multiplexer.

use clap::{Parser, Subcommand, ValueEnum};
use multiplexer::{stop, submit, Multiplexer};
use std::path::PathBuf;
use std::process::ExitCode;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Log level for the multiplexer.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum LogLevel {
    /// No logging
    Off,
    /// Error messages only
    Error,
    /// Warnings and errors
    Warn,
    /// Informational messages (default)
    #[default]
    Info,
    /// Debug messages
    Debug,
    /// Trace messages (very verbose)
    Trace,
}

const AGENT_PROTOCOL: &str = include_str!("../AGENT_PROTOCOL.md");

#[derive(Parser)]
#[command(name = "multiplexer")]
#[command(about = "Multiplexer for managing agent pools")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the multiplexer server
    Start {
        /// Root directory for the multiplexer
        root: PathBuf,
        /// Log level
        #[arg(short, long, default_value = "info")]
        log_level: LogLevel,
    },
    /// Stop a running multiplexer server
    Stop {
        /// Root directory where the server is running
        root: PathBuf,
    },
    /// Submit a task and wait for the result
    Submit {
        /// Root directory where the server is running
        root: PathBuf,
        /// Task input to send
        input: String,
    },
    /// Print the agent protocol documentation
    Protocol,
}

fn init_tracing(level: LogLevel) {
    let filter = match level {
        LogLevel::Off => EnvFilter::new("off"),
        LogLevel::Error => EnvFilter::new("error"),
        LogLevel::Warn => EnvFilter::new("warn"),
        LogLevel::Info => EnvFilter::new("info"),
        LogLevel::Debug => EnvFilter::new("debug"),
        LogLevel::Trace => EnvFilter::new("trace"),
    };

    tracing_subscriber::registry()
        .with(fmt::layer().without_time().with_target(false))
        .with(filter)
        .init();
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Start { root, log_level } => {
            init_tracing(log_level);

            let mut multiplexer = match Multiplexer::new(&root) {
                Ok(m) => m,
                Err(e) => {
                    eprintln!("Failed to start: {e}");
                    return ExitCode::FAILURE;
                }
            };

            if let Err(e) = multiplexer.run() {
                eprintln!("Server error: {e}");
                return ExitCode::FAILURE;
            }
        }
        Command::Stop { root } => {
            if let Err(e) = stop(&root) {
                eprintln!("Failed to stop: {e}");
                return ExitCode::FAILURE;
            }
            eprintln!("Server stopped");
        }
        Command::Submit { root, input } => match submit(&root, &input) {
            Ok(output) => {
                print!("{output}");
            }
            Err(e) => {
                eprintln!("Submit error: {e}");
                return ExitCode::FAILURE;
            }
        },
        Command::Protocol => {
            print!("{AGENT_PROTOCOL}");
        }
    }

    ExitCode::SUCCESS
}
