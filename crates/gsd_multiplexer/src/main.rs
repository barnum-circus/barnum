//! CLI for the GSD multiplexer.

use clap::{Parser, Subcommand};
use gsd_multiplexer::{stop, submit, Multiplexer};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser)]
#[command(name = "gsd_multiplexer")]
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
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Start { root } => {
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
    }

    ExitCode::SUCCESS
}
