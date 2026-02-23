//! CLI for the agent pool.

// CLI binaries legitimately use print/eprintln for user output
#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use agent_pool::{
    AGENTS_DIR, DaemonConfig, Payload, RESPONSE_FILE, TASK_FILE,
    cleanup_stopped, generate_id, id_to_path, list_pools, resolve_pool,
    run_with_config, stop, submit, submit_file,
};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use std::{fs, thread};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

const AGENT_PROTOCOL: &str = include_str!("../../agent_pool/AGENT_PROTOCOL.md");

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

/// Notification mechanism for communicating with the daemon.
#[derive(Debug, Clone, Copy, Default, ValueEnum)]
enum NotifyMethod {
    /// Socket-based RPC (faster, but blocked in sandboxed environments)
    #[default]
    Socket,
    /// File-based events (works in sandboxed environments)
    File,
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
        /// How long an idle agent can wait before being deregistered (in seconds).
        /// Agents that are still alive will re-register by calling `get_task` again.
        #[arg(long, default_value = "60")]
        idle_agent_timeout_secs: u64,
        /// Default timeout for tasks in seconds.
        #[arg(long, default_value = "300")]
        task_timeout_secs: u64,
        /// Disable heartbeat checks for idle agents.
        #[arg(long)]
        no_heartbeat: bool,
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
        /// Task content as inline string
        #[arg(long)]
        data: Option<String>,
        /// Path to file containing task JSON (daemon reads the file)
        #[arg(long)]
        file: Option<PathBuf>,
        /// Notification mechanism: socket (default, faster) or file (works in sandboxes)
        #[arg(long, default_value = "socket")]
        notify: NotifyMethod,
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
        /// Agent name (must be unique within the pool)
        #[arg(long)]
        name: String,
    },
    /// Register as an agent and wait for first task (alias for get_task)
    #[command(name = "register")]
    Register {
        /// Pool ID or path
        #[arg(long)]
        pool: String,
        /// Agent name (must be unique within the pool)
        #[arg(long)]
        name: String,
    },
    /// Submit response to current task and wait for next task
    #[command(name = "next_task")]
    NextTask {
        /// Pool ID or path
        #[arg(long)]
        pool: String,
        /// Agent name
        #[arg(long)]
        name: String,
        /// Response content as inline string
        #[arg(long)]
        data: Option<String>,
        /// Path to file containing response
        #[arg(long)]
        file: Option<PathBuf>,
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

/// Wait for a task file to appear and return the formatted output JSON.
fn wait_for_task(task_file: &std::path::Path, response_file: &std::path::Path, name: &str) -> Result<String, String> {
    loop {
        if task_file.exists() {
            let raw = fs::read_to_string(task_file)
                .map_err(|e| format!("Failed to read task: {e}"))?;

            let envelope: serde_json::Value = serde_json::from_str(&raw)
                .map_err(|e| format!("Failed to parse task envelope: {e}"))?;

            let kind = envelope.get("kind").and_then(|k| k.as_str()).unwrap_or("Task");
            let content = envelope.get("task").cloned().unwrap_or(serde_json::Value::Null);

            let output = serde_json::json!({
                "kind": kind,
                "agent_name": name,
                "response_file": response_file.display().to_string(),
                "content": content
            });

            return Ok(serde_json::to_string_pretty(&output).unwrap_or_default());
        }

        thread::sleep(Duration::from_millis(100));
    }
}

#[allow(clippy::too_many_lines)]
fn main() -> ExitCode {
    let cli = Cli::parse();

    match cli.command {
        Command::Start {
            pool,
            log_level,
            json,
            idle_agent_timeout_secs,
            task_timeout_secs,
            no_heartbeat,
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
                idle_agent_timeout: Duration::from_secs(idle_agent_timeout_secs),
                default_task_timeout: Duration::from_secs(task_timeout_secs),
                heartbeat_enabled: !no_heartbeat,
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
        Command::SubmitTask { pool, data, file, notify } => {
            let root = resolve_pool(&pool);

            // Build payload from --data (inline) or --file (file reference)
            let payload = match (data, file) {
                (Some(d), None) => Payload::inline(d),
                (None, Some(path)) => Payload::file_ref(path),
                (Some(d), Some(_)) => Payload::inline(d), // --data takes precedence
                (None, None) => {
                    eprintln!("Either --data or --file must be provided");
                    return ExitCode::FAILURE;
                }
            };

            // Send via chosen notification method
            let result = match notify {
                NotifyMethod::Socket => submit(&root, &payload),
                NotifyMethod::File => submit_file(&root, &payload),
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
        Command::Protocol { pool } => {
            let mut output = AGENT_PROTOCOL.to_string();

            if let Some(id) = &pool {
                let path = id_to_path(id);
                output = output
                    .replace("<POOL_ID>", id)
                    .replace("abc12345", id)
                    .replace("/tmp/gsd/<POOL_ID>", &path.display().to_string());
            }

            print!("{output}");
        }
        Command::DeregisterAgent { pool, name } => {
            let root = resolve_pool(&pool);
            let agent_dir = root.join(AGENTS_DIR).join(&name);

            // Remove the agent directory if it exists
            if agent_dir.exists()
                && let Err(e) = fs::remove_dir_all(&agent_dir)
            {
                eprintln!("Failed to remove agent directory: {e}");
                return ExitCode::FAILURE;
            }

            eprintln!("Deregistered agent '{name}'");
        }
        Command::GetTask { pool, name } | Command::Register { pool, name } => {
            let root = resolve_pool(&pool);
            let agent_dir = root.join(AGENTS_DIR).join(&name);

            if let Err(e) = fs::create_dir_all(&agent_dir) {
                eprintln!("Failed to create agent directory: {e}");
                return ExitCode::FAILURE;
            }

            let task_file = agent_dir.join(TASK_FILE);
            let response_file = agent_dir.join(RESPONSE_FILE);

            match wait_for_task(&task_file, &response_file, &name) {
                Ok(output) => println!("{output}"),
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Command::NextTask { pool, name, data, file } => {
            let root = resolve_pool(&pool);
            let agent_dir = root.join(AGENTS_DIR).join(&name);

            if !agent_dir.exists() {
                eprintln!("Agent not registered. Use 'register' first.");
                return ExitCode::FAILURE;
            }

            let task_file = agent_dir.join(TASK_FILE);
            let response_file = agent_dir.join(RESPONSE_FILE);

            // Get response content from --data or --file
            let response_content = match (data, file) {
                (Some(d), None) => d,
                (None, Some(path)) => match fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("Failed to read response file {}: {e}", path.display());
                        return ExitCode::FAILURE;
                    }
                },
                (Some(d), Some(_)) => d,
                (None, None) => {
                    eprintln!("Either --data or --file must be provided");
                    return ExitCode::FAILURE;
                }
            };

            // Write response to current task
            if let Err(e) = fs::write(&response_file, &response_content) {
                eprintln!("Failed to write response: {e}");
                return ExitCode::FAILURE;
            }

            // Wait for daemon to consume the response (task file removed)
            while task_file.exists() {
                thread::sleep(Duration::from_millis(100));
            }

            // Wait for next task
            match wait_for_task(&task_file, &response_file, &name) {
                Ok(output) => println!("{output}"),
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    ExitCode::SUCCESS
}
