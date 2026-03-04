//! CLI for the agent pool.

// CLI binaries legitimately use print/eprintln for user output
#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use agent_pool::{
    AGENTS_DIR, DaemonConfig, Payload, STATUS_FILE, TaskAssignment, VerifiedWatcher,
    default_pool_root, generate_id, id_to_path, is_daemon_running, list_pools, resolve_pool,
    response_path, run_with_config, stop, submit, submit_file, submit_file_with_timeout,
    wait_for_task,
};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Duration;
use std::{fs, thread};
use tracing::info;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

const AGENT_PROTOCOL: &str = include_str!("../../agent_pool/protocols/AGENT_PROTOCOL.md");
const LOW_LEVEL_PROTOCOL: &str = include_str!("../../agent_pool/protocols/LOW_LEVEL_PROTOCOL.md");
const VERSION: &str = env!("AGENT_POOL_VERSION");

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
    /// Base directory for pools. Pool IDs resolve to `<pool-root>/<id>/`.
    /// Defaults to `/tmp/agent_pool` on Unix.
    #[arg(long, global = true)]
    pool_root: Option<PathBuf>,

    /// Log level
    #[arg(short, long, global = true, default_value = "info")]
    log_level: LogLevel,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Start the agent pool server
    Start {
        /// Pool ID. If omitted, generates a new ID.
        /// IDs resolve to `<pool-root>/<id>/` (default: `/tmp/agent_pool/<id>/`)
        #[arg(long)]
        pool: Option<String>,
        /// Output pool info as JSON (for scripts)
        #[arg(long)]
        json: bool,
        /// How long an idle worker can wait before receiving a heartbeat (in seconds).
        #[arg(long, default_value = "180")]
        idle_timeout_secs: u64,
        /// Default timeout for tasks in seconds.
        #[arg(long, default_value = "300")]
        task_timeout_secs: u64,
        /// Disable all heartbeats for idle workers.
        #[arg(long)]
        no_heartbeat: bool,
        /// Disable periodic heartbeats (still send initial heartbeat on registration).
        #[arg(long)]
        no_periodic_heartbeat: bool,
        /// Disable initial heartbeat on registration (still send periodic heartbeats).
        #[arg(long)]
        no_initial_heartbeat: bool,
        /// Stop existing daemon before starting (if running).
        /// Without this flag, starting fails if a daemon is already running.
        #[arg(long)]
        stop: bool,
    },
    /// Stop a running agent pool server
    Stop {
        /// Pool ID (not a path)
        #[arg(long)]
        pool: String,
    },
    /// Submit a task and wait for the result
    #[command(name = "submit_task")]
    SubmitTask {
        /// Pool ID (not a path)
        #[arg(long)]
        pool: String,
        /// Task content as inline string
        #[arg(long, conflicts_with = "file")]
        data: Option<String>,
        /// Path to file containing task JSON (daemon reads the file)
        #[arg(long, conflicts_with = "data")]
        file: Option<PathBuf>,
        /// Notification mechanism: socket (default, faster) or file (works in sandboxes)
        #[arg(long, default_value = "socket")]
        notify: NotifyMethod,
        /// Timeout in seconds (default: 300 for file notify, varies for socket)
        #[arg(long)]
        timeout_secs: Option<u64>,
    },
    /// List all pools
    List,
    /// Print the agent protocol documentation
    Protocol {
        /// Pool ID to include in the instructions
        #[arg(long)]
        pool: Option<String>,
        /// Agent name to include in the instructions
        #[arg(long)]
        name: Option<String>,
        /// Show low-level file/socket protocol (for debugging/internals)
        #[arg(long)]
        low_level: bool,
    },
    /// Wait for and return the next task (for agents)
    #[command(name = "get_task")]
    GetTask {
        /// Pool ID (not a path)
        #[arg(long)]
        pool: String,
        /// Agent name (optional, for debugging)
        #[arg(long)]
        name: Option<String>,
    },
    /// Print version information
    Version {
        /// Output as JSON (for programmatic access)
        #[arg(long)]
        json: bool,
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
        .with(fmt::layer().with_writer(std::io::stderr).with_target(false))
        .with(filter)
        .init();
}

/// Format a task assignment as JSON output.
fn format_task_output(
    assignment: &TaskAssignment,
    pool_root: &std::path::Path,
    name: Option<&str>,
) -> String {
    let agents_dir = pool_root.join(AGENTS_DIR);
    let response_file = response_path(&agents_dir, &assignment.uuid);

    // Parse the task envelope
    let envelope: serde_json::Value =
        serde_json::from_str(&assignment.content).unwrap_or(serde_json::Value::Null);

    let kind = envelope
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("Task");
    let content = envelope
        .get("task")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let instructions = envelope.get("instructions").cloned();

    let mut output = serde_json::json!({
        "uuid": assignment.uuid,
        "kind": kind,
        "response_file": response_file.display().to_string(),
        "content": content
    });

    if let Some(n) = name {
        output["agent_name"] = serde_json::Value::String(n.to_string());
    }
    if let Some(inst) = instructions {
        output["instructions"] = inst;
    }

    serde_json::to_string_pretty(&output).unwrap_or_default()
}

#[allow(clippy::too_many_lines)]
fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.log_level);
    let pool_root = cli.pool_root.clone().unwrap_or_else(default_pool_root);
    info!(
        command = ?cli.command,
        pool_root = %pool_root.display(),
        log_level = ?cli.log_level,
        "CLI invoked"
    );

    match cli.command {
        Command::Start {
            pool,
            json,
            idle_timeout_secs,
            task_timeout_secs,
            no_heartbeat,
            no_periodic_heartbeat,
            no_initial_heartbeat,
            stop: stop_flag,
        } => {
            // Validate pool ID if provided
            if let Some(ref p) = pool
                && let Err(e) = validate_pool_id(p)
            {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }

            // Resolve pool ID or generate new one
            let id = pool.unwrap_or_else(generate_id);
            let root = id_to_path(&pool_root, &id);

            // TODO: Pass periodic/initial heartbeat flags to DaemonConfig when supported
            // For now, --no-heartbeat disables all; finer-grained control requires DaemonConfig changes
            let _ = (no_periodic_heartbeat, no_initial_heartbeat); // Silence unused warnings

            // Handle existing daemon/directory
            if root.exists() {
                let daemon_running = is_daemon_running(&root);

                if daemon_running {
                    if stop_flag {
                        // --stop: Stop daemon first
                        if let Err(e) = stop(&root) {
                            eprintln!("Failed to stop daemon: {e}");
                            return ExitCode::FAILURE;
                        }
                        // Wait for daemon to actually exit
                        for _ in 0..50 {
                            if !is_daemon_running(&root) {
                                break;
                            }
                            thread::sleep(Duration::from_millis(100));
                        }
                        if is_daemon_running(&root) {
                            eprintln!("Daemon did not exit in time");
                            return ExitCode::FAILURE;
                        }
                        eprintln!("Stopped existing daemon");
                    } else {
                        // No --stop flag, daemon running
                        eprintln!("Daemon is already running. Use --stop to stop and restart.");
                        return ExitCode::FAILURE;
                    }
                }

                // Always wipe the directory (daemon is now stopped or wasn't running)
                if let Err(e) = fs::remove_dir_all(&root) {
                    eprintln!("Failed to clear pool directory: {e}");
                    return ExitCode::FAILURE;
                }
                eprintln!("Cleared pool directory");
            }

            // Print pool info
            if json {
                let info = serde_json::json!({ "id": id });
                println!("{}", serde_json::to_string(&info).unwrap_or_default());
            } else {
                eprintln!("Starting pool {id}");
            }

            let config = DaemonConfig {
                idle_timeout: Duration::from_secs(idle_timeout_secs),
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
            if let Err(e) = validate_pool_id(&pool) {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
            let root = resolve_pool(&pool_root, &pool);
            if let Err(e) = stop(&root) {
                eprintln!("Failed to stop: {e}");
                return ExitCode::FAILURE;
            }
            eprintln!("Server stopped");
        }
        Command::SubmitTask {
            pool,
            data,
            file,
            notify,
            timeout_secs,
        } => {
            if let Err(e) = validate_pool_id(&pool) {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }
            let root = resolve_pool(&pool_root, &pool);

            // Create watcher at CLI entry point
            let mut watcher = match VerifiedWatcher::new(&root, std::slice::from_ref(&root)) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("Failed to create watcher: {e}");
                    return ExitCode::FAILURE;
                }
            };

            // Build payload from --data (inline) or --file (file reference)
            let payload = match (data, file) {
                (Some(d), _) => Payload::inline(d),
                (None, Some(path)) => Payload::file_ref(path),
                (None, None) => {
                    eprintln!("Either --data or --file must be provided");
                    return ExitCode::FAILURE;
                }
            };

            // Send via chosen notification method
            let result = match (notify, timeout_secs) {
                (NotifyMethod::Socket, _) => submit(&mut watcher, &root, &payload),
                (NotifyMethod::File, Some(secs)) => submit_file_with_timeout(
                    &mut watcher,
                    &root,
                    &payload,
                    Duration::from_secs(secs),
                ),
                (NotifyMethod::File, None) => submit_file(&mut watcher, &root, &payload),
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
        Command::List => match list_pools(&pool_root) {
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
        Command::Protocol {
            pool,
            name,
            low_level,
        } => {
            let mut output = if low_level {
                LOW_LEVEL_PROTOCOL.to_string()
            } else {
                AGENT_PROTOCOL.to_string()
            };

            if let Some(id) = &pool {
                let path = id_to_path(&pool_root, id);
                output = output
                    .replace("<POOL_ID>", id)
                    .replace("abc12345", id)
                    .replace("/tmp/agent_pool/<POOL_ID>", &path.display().to_string());
            }

            if let Some(n) = &name {
                output = output
                    .replace("<AGENT_NAME>", n)
                    .replace("--name <AGENT_NAME>", &format!("--name {n}"));
            }

            print!("{output}");
        }
        Command::GetTask { pool, name } => {
            if let Err(e) = validate_pool_id(&pool) {
                eprintln!("{e}");
                return ExitCode::FAILURE;
            }

            let root = resolve_pool(&pool_root, &pool);

            // Create watcher at CLI entry point
            // Single canary at root - directories already exist (daemon created them)
            let mut watcher = match VerifiedWatcher::new(&root, std::slice::from_ref(&root)) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("Failed to create watcher: {e}");
                    return ExitCode::FAILURE;
                }
            };

            // Wait for daemon to be ready (status file signals readiness after sync)
            let status_file = root.join(STATUS_FILE);
            if !wait_for_status_file(&status_file) {
                eprintln!("Daemon not ready (status file not found within timeout)");
                return ExitCode::FAILURE;
            }

            // Wait for task assignment using the new anonymous worker protocol
            match wait_for_task(&mut watcher, &root, name.as_deref(), None) {
                Ok(assignment) => {
                    let output = format_task_output(&assignment, &root, name.as_deref());
                    println!("{output}");
                }
                Err(e) => {
                    eprintln!("Failed to get task: {e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Command::Version { json } => {
            if json {
                println!(r#"{{"version": "{VERSION}"}}"#);
            } else {
                println!("{VERSION}");
            }
        }
    }

    ExitCode::SUCCESS
}

/// Validate that a pool ID is not a path.
/// Pool IDs should be simple identifiers, not paths.
fn validate_pool_id(pool: &str) -> Result<(), String> {
    if pool.contains('/') || pool.contains('\\') {
        return Err(format!(
            "Pool ID cannot contain path separators. Got: '{pool}'. Use --pool-root to specify the base directory."
        ));
    }
    Ok(())
}

/// Wait for the status file to appear (daemon ready signal).
/// Returns true if found within timeout, false otherwise.
fn wait_for_status_file(status_file: &std::path::Path) -> bool {
    const TIMEOUT: Duration = Duration::from_secs(5);
    const POLL_INTERVAL: Duration = Duration::from_millis(100);

    let start = std::time::Instant::now();
    while start.elapsed() < TIMEOUT {
        if status_file.exists() {
            return true;
        }
        thread::sleep(POLL_INTERVAL);
    }
    false
}
