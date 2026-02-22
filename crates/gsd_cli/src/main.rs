//! GSD CLI - Get Sh*** Done.
//!
//! Command-line interface for the GSD JSON-based task orchestrator.

#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use clap::{Parser, Subcommand};
use gsd_json::{CompiledSchemas, Config, RunnerConfig, Task, generate_full_docs, run};
use std::fs::File;
use std::io;
use std::path::PathBuf;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

#[derive(Parser)]
#[command(name = "gsd")]
#[command(about = "Get Sh*** Done - JSON-based task orchestrator")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the state machine with a config file
    Run {
        /// Path to the GSD config file
        config: PathBuf,

        /// Agent pool root directory (default: temp directory)
        #[arg(long)]
        root: Option<PathBuf>,

        /// Wake script to call before starting
        #[arg(long)]
        wake: Option<String>,

        /// Initial tasks (JSON array string or path to file)
        #[arg(long)]
        initial: Option<String>,

        /// Log file path (logs emitted in addition to stderr)
        #[arg(long)]
        log_file: Option<PathBuf>,
    },

    /// Generate markdown documentation from config
    Docs {
        /// Path to the GSD config file
        config: PathBuf,
    },

    /// Validate a config file
    Validate {
        /// Path to the GSD config file
        config: PathBuf,
    },
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Command::Run {
            config,
            root,
            wake,
            initial,
            log_file,
        } => {
            // Initialize tracing with optional log file
            init_tracing(log_file.as_ref())?;
            let cfg = Config::load(&config)?;
            cfg.validate()
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;

            let config_dir = config.parent().unwrap_or_else(|| std::path::Path::new("."));
            let schemas = CompiledSchemas::compile(&cfg, config_dir)?;

            // Parse initial tasks
            let initial_tasks = parse_initial_tasks(initial)?;

            // Determine agent_pool root
            let pool_root = root.unwrap_or_else(|| {
                let temp = std::env::temp_dir().join("gsd-pool");
                std::fs::create_dir_all(&temp).ok();
                temp
            });

            let runner_config = RunnerConfig {
                agent_pool_root: &pool_root,
                wake_script: wake.as_deref(),
                initial_tasks,
            };

            run(&cfg, &schemas, runner_config)?;
        }

        Command::Docs { config } => {
            let cfg = Config::load(&config)?;
            let docs = generate_full_docs(&cfg);
            print!("{docs}");
        }

        Command::Validate { config } => {
            let cfg = Config::load(&config)?;
            match cfg.validate() {
                Ok(()) => {
                    println!("Config is valid.");
                    println!("Steps: {}", cfg.steps.len());
                    for step in &cfg.steps {
                        println!(
                            "  {} -> {}",
                            step.name,
                            if step.next.is_empty() {
                                "(terminal)".to_string()
                            } else {
                                step.next.join(", ")
                            }
                        );
                    }
                }
                Err(e) => {
                    eprintln!("Config validation failed: {e}");
                    return Err(io::Error::new(io::ErrorKind::InvalidData, e));
                }
            }
        }
    }

    Ok(())
}

fn parse_initial_tasks(initial: Option<String>) -> io::Result<Vec<Task>> {
    let Some(s) = initial else {
        return Ok(Vec::new());
    };

    // Check if it's a file path
    let json_str = {
        let path = PathBuf::from(&s);
        if path.exists() {
            std::fs::read_to_string(path)?
        } else {
            // Assume it's inline JSON
            s
        }
    };

    serde_json::from_str(&json_str).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid initial tasks JSON: {e}"),
        )
    })
}

fn init_tracing(log_file: Option<&PathBuf>) -> io::Result<()> {
    let filter =
        EnvFilter::from_default_env().add_directive("gsd=info".parse().unwrap_or_default());

    let stderr_layer = fmt::layer().without_time().with_target(false);

    if let Some(path) = log_file {
        let file = File::create(path)?;
        let file_layer = fmt::layer()
            .with_ansi(false)
            .with_writer(file)
            .with_target(true);

        tracing_subscriber::registry()
            .with(filter)
            .with(stderr_layer)
            .with(file_layer)
            .init();
    } else {
        tracing_subscriber::registry()
            .with(filter)
            .with(stderr_layer)
            .init();
    }

    Ok(())
}
