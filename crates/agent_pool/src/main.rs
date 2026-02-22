//! CLI for the agent pool.

// CLI binaries legitimately use print/eprintln for user output
#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use agent_pool::{
    AGENTS_DIR, DaemonConfig, RESPONSE_FILE, TASK_FILE,
    cleanup_stopped, generate_id, id_to_path, list_pools, resolve_pool, run_with_config, stop,
    submit, submit_file,
};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;
use std::{fs, thread};
use std::time::Duration;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

const AGENT_PROTOCOL: &str = include_str!("../AGENT_PROTOCOL.md");

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
        /// Pool ID or path. If omitted, generates a new ID.
        /// IDs resolve to /tmp/gsd/<id>/
        #[arg(long)]
        pool: Option<String>,
        /// Log level
        #[arg(short, long, default_value = "info")]
        log_level: LogLevel,
        /// Output pool info as JSON (for scripts)
        #[arg(long)]
        json: bool,
        /// Send health check when agent first registers
        #[arg(long, default_value = "false")]
        initial_health_check: bool,
        /// Send periodic health checks to idle agents
        #[arg(long, default_value = "false")]
        periodic_health_check: bool,
        /// Health check interval in seconds (how often to check idle agents)
        #[arg(long, default_value = "60")]
        health_check_interval_secs: u64,
        /// Health check timeout in seconds (how long to wait for response)
        #[arg(long, default_value = "30")]
        health_check_timeout_secs: u64,
    },
    /// Stop a running agent pool server
    Stop {
        /// Pool ID or path
        #[arg(long)]
        pool: String,
    },
    /// Submit a task and wait for the result
    #[command(name = "submit_task")]
    SubmitTask {
        /// Pool ID or path
        #[arg(long)]
        pool: String,
        /// Task input as inline JSON (uses socket protocol)
        #[arg(long, conflicts_with = "file")]
        input: Option<String>,
        /// Path to file containing task input JSON (uses file protocol, works in sandboxes)
        #[arg(long, conflicts_with = "input")]
        file: Option<PathBuf>,
    },
    /// List all pools
    List,
    /// Clean up stopped pools
    Cleanup,
    /// Print the agent protocol documentation
    Protocol {
        /// Pool ID to include in the instructions
        #[arg(long)]
        pool: Option<String>,
        /// Agent name to include in the instructions
        #[arg(long)]
        name: Option<String>,
    },
    /// Deregister an agent from the pool
    #[command(name = "deregister_agent")]
    DeregisterAgent {
        /// Pool ID or path
        #[arg(long)]
        pool: String,
        /// Agent name
        #[arg(long)]
        name: String,
    },
    /// Wait for and return the next task (for agents)
    #[command(name = "get_task")]
    GetTask {
        /// Pool ID or path
        #[arg(long)]
        pool: String,
        /// Agent name
        #[arg(long)]
        name: String,
        /// Automatically respond to health checks (agent never sees them).
        /// Default: false - agents should see and respond to health checks
        /// to demonstrate they're still actively listening.
        #[arg(long, default_value = "false")]
        auto_health_check: bool,
    },
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
        .with(fmt::layer().with_target(false))
        .with(filter)
        .init();
}

#[allow(clippy::too_many_lines)]
fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Start {
            pool,
            log_level,
            json,
            initial_health_check,
            periodic_health_check,
            health_check_interval_secs,
            health_check_timeout_secs,
        } => {
            init_tracing(log_level);

            // Resolve pool reference or generate new ID
            let (id, root) = match pool {
                Some(p) if p.contains('/') => {
                    // It's a path
                    (None, PathBuf::from(p))
                }
                Some(id) => {
                    // It's an ID
                    (Some(id.clone()), id_to_path(&id))
                }
                None => {
                    // Generate new ID
                    let id = generate_id();
                    (Some(id.clone()), id_to_path(&id))
                }
            };

            // Print pool info
            if json {
                let info = serde_json::json!({ "id": id });
                println!("{}", serde_json::to_string(&info).unwrap_or_default());
            } else if let Some(id) = &id {
                eprintln!("Starting pool {id}");
            } else {
                eprintln!("Starting pool");
            }

            let config = DaemonConfig {
                initial_health_check,
                periodic_health_check,
                health_check_interval: Duration::from_secs(health_check_interval_secs),
                health_check_timeout: Duration::from_secs(health_check_timeout_secs),
            };

            // run_with_config() returns Result<Infallible, _>, so Ok case never happens
            match run_with_config(&root, config) {
                Ok(never) => match never {},
                Err(e) => {
                    eprintln!("Server error: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Command::Stop { pool } => {
            let root = resolve_pool(&pool);
            if let Err(e) = stop(&root) {
                eprintln!("Failed to stop: {e}");
                return ExitCode::FAILURE;
            }
            eprintln!("Server stopped");
        }
        Command::SubmitTask { pool, input, file } => {
            let root = resolve_pool(&pool);

            // --input uses socket protocol, --file uses file protocol
            let result = match (input, file) {
                (Some(task_input), None) => submit(&root, &task_input),
                (None, Some(path)) => {
                    let task_input = match fs::read_to_string(&path) {
                        Ok(content) => content,
                        Err(e) => {
                            eprintln!("Failed to read file {}: {e}", path.display());
                            return ExitCode::FAILURE;
                        }
                    };
                    submit_file(&root, &task_input)
                }
                (None, None) => {
                    eprintln!("Either --input or --file must be provided");
                    return ExitCode::FAILURE;
                }
                (Some(_), Some(_)) => unreachable!("clap prevents this"),
            };

            match result {
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
            }
        }
        Command::List => match list_pools() {
            Ok(pools) => {
                if pools.is_empty() {
                    eprintln!("No pools found");
                } else {
                    println!("{:<12} {:<8} PATH", "ID", "STATUS");
                    for pool in pools {
                        let status = if pool.running { "running" } else { "stopped" };
                        println!("{:<12} {:<8} {}", pool.id, status, pool.path.display());
                    }
                }
            }
            Err(e) => {
                eprintln!("Failed to list pools: {e}");
                return ExitCode::FAILURE;
            }
        },
        Command::Cleanup => match cleanup_stopped() {
            Ok(count) => {
                eprintln!("Cleaned up {count} stopped pool(s)");
            }
            Err(e) => {
                eprintln!("Cleanup failed: {e}");
                return ExitCode::FAILURE;
            }
        },
        Command::Protocol { pool, name } => {
            let mut output = AGENT_PROTOCOL.to_string();

            if let Some(id) = &pool {
                let path = id_to_path(id);
                output = output
                    .replace("<POOL_ID>", id)
                    .replace("abc12345", id)
                    .replace("/tmp/gsd/<POOL_ID>", &path.display().to_string());
            }

            if let Some(agent_name) = &name {
                output = output
                    .replace("<YOUR_NAME>", agent_name)
                    .replace("claude-1", agent_name);
            }

            print!("{output}");
        }
        Command::DeregisterAgent { pool, name } => {
            let root = resolve_pool(&pool);
            let agent_dir = root.join(AGENTS_DIR).join(&name);

            // Remove the agent directory if it exists
            if agent_dir.exists() && let Err(e) = fs::remove_dir_all(&agent_dir) {
                eprintln!("Failed to remove agent directory: {e}");
                return ExitCode::FAILURE;
            }

            eprintln!("Deregistered agent '{name}'");
        }
        Command::GetTask {
            pool,
            name,
            auto_health_check,
        } => {
            let root = resolve_pool(&pool);
            let agent_dir = root.join(AGENTS_DIR).join(&name);

            // Create agent directory if it doesn't exist (registers the agent)
            if let Err(e) = fs::create_dir_all(&agent_dir) {
                eprintln!("Failed to create agent directory: {e}");
                return ExitCode::FAILURE;
            }

            let task_file = agent_dir.join(TASK_FILE);
            let response_file = agent_dir.join(RESPONSE_FILE);

            // Poll for task file
            loop {
                if task_file.exists() {
                    // Read the task envelope (daemon writes {"kind": "...", "content": ...})
                    let raw = match fs::read_to_string(&task_file) {
                        Ok(c) => c,
                        Err(e) => {
                            eprintln!("Failed to read task: {e}");
                            return ExitCode::FAILURE;
                        }
                    };

                    // Parse envelope and extract kind/content
                    let envelope: serde_json::Value = match serde_json::from_str(&raw) {
                        Ok(v) => v,
                        Err(e) => {
                            eprintln!("Failed to parse task envelope: {e}");
                            return ExitCode::FAILURE;
                        }
                    };

                    let kind = envelope
                        .get("kind")
                        .and_then(|k| k.as_str())
                        .unwrap_or("Task");
                    let content = envelope
                        .get("content")
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);

                    // Handle health checks automatically if enabled
                    if auto_health_check && kind == "HealthCheck" {
                        // Respond to health check and continue polling
                        if let Err(e) = fs::write(&response_file, "{}") {
                            eprintln!("Failed to respond to health check: {e}");
                            return ExitCode::FAILURE;
                        }
                        // Wait for daemon to clean up files before polling again
                        thread::sleep(Duration::from_millis(100));
                        continue;
                    }

                    // Output task info with stable response file path
                    let output = serde_json::json!({
                        "kind": kind,
                        "response_file": response_file.display().to_string(),
                        "content": content
                    });

                    println!(
                        "{}",
                        serde_json::to_string_pretty(&output).unwrap_or_default()
                    );
                    return ExitCode::SUCCESS;
                }

                // No task yet, wait and try again
                thread::sleep(Duration::from_millis(100));
            }
        }
    }

    ExitCode::SUCCESS
}
