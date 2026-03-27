//! Barnum workflow engine CLI.

use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(name = "barnum", about = "Barnum workflow engine")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Deserialize a workflow config and print the result.
    Run {
        /// Serialized JSON config.
        #[arg(long)]
        config: String,
    },
}

fn main() {
    let cli = Cli::parse();
    match cli.command {
        Command::Run { config } => {
            if let Err(e) = run(&config) {
                #[expect(clippy::print_stderr)]
                {
                    eprintln!("{e}");
                }
                std::process::exit(1);
            }
        }
    }
}

fn run(input: &str) -> Result<(), Box<dyn std::error::Error>> {
    let config: barnum_ast::Config = serde_json::from_str(input)?;
    let output = serde_json::to_string_pretty(&config)?;
    #[expect(clippy::print_stdout)]
    {
        println!("{output}");
    }
    Ok(())
}
