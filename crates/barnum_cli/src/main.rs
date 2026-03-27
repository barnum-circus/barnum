//! Barnum workflow engine CLI.

use std::io::Read;

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "barnum", about = "Barnum workflow engine")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read a workflow config from stdin, deserialize it, and print the result.
    Run,
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Run => {
            if let Err(e) = run() {
                #[expect(clippy::print_stderr)]
                {
                    eprintln!("{e}");
                }
                std::process::exit(1);
            }
        }
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    let mut input = String::new();
    std::io::stdin().read_to_string(&mut input)?;
    let config: barnum_ast::Config = serde_json::from_str(&input)?;
    let output = serde_json::to_string_pretty(&config)?;
    // Allow print_stdout in the CLI binary — it's the intended output mechanism.
    #[expect(clippy::print_stdout)]
    {
        println!("{output}");
    }
    Ok(())
}
