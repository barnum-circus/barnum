//! CLI for the agent pool.

// CLI binaries legitimately use print/eprintln for user output
#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use agent_pool::{run, stop, submit};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// Log level for the agent pool.
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
#[command(name = "agent_pool")]
#[command(about = "Agent pool for managing workers with file-based task dispatch")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the agent pool server
    Start {
        /// Root directory for the agent pool
        root: PathBuf,
        /// Log level
        #[arg(short, long, default_value = "info")]
        log_level: LogLevel,
    },
    /// Stop a running agent pool server
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

            // run() returns Result<Infallible, _>, so Ok case never happens
            match run(&root) {
                Ok(never) => match never {},
                Err(e) => {
                    eprintln!("Server error: {e}");
                    return ExitCode::FAILURE;
                }
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
            Ok(response) => {
                // Output structured JSON response
                match serde_json::to_string(&response) {
                    Ok(json) => println!("{json}"),
                    Err(e) => {
                        eprintln!("Failed to serialize response: {e}");
                        return ExitCode::FAILURE;
                    }
                }
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
