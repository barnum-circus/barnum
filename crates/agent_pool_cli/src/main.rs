//! CLI for the agent pool.

// CLI binaries legitimately use print/eprintln for user output
#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use agent_pool::{
    AGENTS_DIR, AgentEvent, DaemonConfig, Payload, RESPONSE_FILE, STATUS_FILE, TASK_FILE,
    Transport, cleanup_stopped, create_watcher, default_pool_root, generate_id, id_to_path,
    is_daemon_running, list_pools, resolve_pool, run_with_config, stop, submit, submit_file,
    submit_file_with_timeout, verify_watcher_sync, wait_for_task,
};
use clap::{Parser, Subcommand, ValueEnum};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::mpsc;
use std::time::Duration;
use std::{fs, thread};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

const AGENT_PROTOCOL: &str = include_str!("../../agent_pool/AGENT_PROTOCOL.md");
const LOW_LEVEL_PROTOCOL: &str = include_str!("../../agent_pool/LOW_LEVEL_PROTOCOL.md");

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

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Start the agent pool server
    Start {
        /// Pool ID or path. If omitted, generates a new ID.
        /// IDs resolve to `<pool-root>/<id>/` (default: `/tmp/agent_pool/<id>/`)
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
        /// Disable all heartbeats (both immediate and periodic).
        #[arg(long, conflicts_with_all = ["no_immediate_heartbeat", "no_periodic_heartbeat"])]
        no_heartbeat: bool,
        /// Disable immediate heartbeat on agent connect.
        #[arg(long, conflicts_with_all = ["no_heartbeat", "no_periodic_heartbeat"])]
        no_immediate_heartbeat: bool,
        /// Disable periodic heartbeats after idle timeout.
        #[arg(long, conflicts_with_all = ["no_heartbeat", "no_immediate_heartbeat"])]
        no_periodic_heartbeat: bool,
        /// Stop existing daemon before starting (if running).
        /// Without this flag, starting fails if a daemon is already running.
        #[arg(long)]
        stop: bool,
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
    /// Clean up stopped pools
    Cleanup,
    /// Print the agent protocol documentation
    Protocol {
        /// Pool ID to include in the instructions
        #[arg(long)]
        pool: Option<String>,
        /// Show low-level file/socket protocol (for debugging/internals)
        #[arg(long)]
        low_level: bool,
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
    /// Register as an agent and wait for first task
    #[command(name = "register")]
    Register {
        /// Pool ID or path
        #[arg(long)]
        pool: String,
        /// Agent name (must be unique within the pool)
        #[arg(long)]
        name: String,
        /// Log level
        #[arg(short, long, default_value = "off")]
        log_level: LogLevel,
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
        #[arg(long, conflicts_with = "file")]
        data: Option<String>,
        /// Path to file containing response
        #[arg(long, conflicts_with = "data")]
        file: Option<PathBuf>,
        /// Log level
        #[arg(short, long, default_value = "off")]
        log_level: LogLevel,
        /// Submit response and deregister (exit cleanly without waiting for next task)
        #[arg(long)]
        deregister: bool,
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

/// Wait for a task file to appear and return the formatted output JSON.
///
/// Uses notify-based file watching instead of polling.
fn wait_and_read_task(
    transport: &Transport,
    events_rx: &mpsc::Receiver<AgentEvent>,
    name: &str,
) -> Result<String, String> {
    // Wait for task using notify (no polling!)
    wait_for_task(transport, events_rx).map_err(|e| format!("Wait error: {e}"))?;

    // Read task using Transport
    let raw = transport
        .read(TASK_FILE)
        .map_err(|e| format!("Failed to read task: {e}"))?;

    let envelope: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("Failed to parse task envelope: {e}"))?;

    let kind = envelope
        .get("kind")
        .and_then(|k| k.as_str())
        .unwrap_or("Task");
    let content = envelope
        .get("task")
        .cloned()
        .unwrap_or(serde_json::Value::Null);
    let instructions = envelope.get("instructions").cloned();

    let response_file = transport
        .path()
        .map(|p| p.join(RESPONSE_FILE))
        .ok_or("Socket transport not supported")?;

    let mut output = serde_json::json!({
        "kind": kind,
        "agent_name": name,
        "response_file": response_file.display().to_string(),
        "content": content
    });
    if let Some(inst) = instructions {
        output["instructions"] = inst;
    }

    Ok(serde_json::to_string_pretty(&output).unwrap_or_default())
}

#[allow(clippy::too_many_lines)]
fn main() -> ExitCode {
    let cli = Cli::parse();
    let pool_root = cli.pool_root.unwrap_or_else(default_pool_root);

    match cli.command {
        Command::Start {
            pool,
            log_level,
            json,
            idle_agent_timeout_secs,
            task_timeout_secs,
            no_heartbeat,
            no_immediate_heartbeat,
            no_periodic_heartbeat,
            stop: stop_flag,
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
                    (Some(id.clone()), id_to_path(&pool_root, &id))
                }
                None => {
                    // Generate new ID
                    let id = generate_id();
                    (Some(id.clone()), id_to_path(&pool_root, &id))
                }
            };

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
            } else if let Some(id) = &id {
                eprintln!("Starting pool {id}");
            } else {
                eprintln!("Starting pool");
            }

            let config = DaemonConfig {
                idle_agent_timeout: Duration::from_secs(idle_agent_timeout_secs),
                default_task_timeout: Duration::from_secs(task_timeout_secs),
                immediate_heartbeat_enabled: !no_heartbeat && !no_immediate_heartbeat,
                periodic_heartbeat_enabled: !no_heartbeat && !no_periodic_heartbeat,
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
            let root = resolve_pool(&pool_root, &pool);

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
                (NotifyMethod::Socket, _) => submit(&root, &payload),
                (NotifyMethod::File, Some(secs)) => {
                    submit_file_with_timeout(&root, &payload, Duration::from_secs(secs))
                }
                (NotifyMethod::File, None) => submit_file(&root, &payload),
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
        Command::Cleanup => match cleanup_stopped(&pool_root) {
            Ok(count) => {
                eprintln!("Cleaned up {count} stopped pool(s)");
            }
            Err(e) => {
                eprintln!("Cleanup failed: {e}");
                return ExitCode::FAILURE;
            }
        },
        Command::Protocol { pool, low_level } => {
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

            print!("{output}");
        }
        Command::DeregisterAgent { pool, name } => {
            let root = resolve_pool(&pool_root, &pool);
            let agent_dir = root.join(AGENTS_DIR).join(&name);

            if !agent_dir.exists() {
                eprintln!("Agent '{name}' not found");
                return ExitCode::SUCCESS;
            }

            // Write a Kicked message so any waiting CLI exits cleanly
            let kicked = serde_json::json!({
                "kind": "Kicked",
                "reason": "Deregistered"
            });
            if let Err(e) = fs::write(agent_dir.join(TASK_FILE), kicked.to_string()) {
                eprintln!("Warning: failed to write Kicked message: {e}");
            }

            // Give the CLI a moment to see the Kicked message
            thread::sleep(Duration::from_millis(50));

            // Remove the agent directory
            if let Err(e) = fs::remove_dir_all(&agent_dir) {
                eprintln!("Failed to remove agent directory: {e}");
                return ExitCode::FAILURE;
            }

            eprintln!("Deregistered agent '{name}'");
        }
        Command::Register {
            pool,
            name,
            log_level,
        } => {
            init_tracing(log_level);

            let root = resolve_pool(&pool_root, &pool);

            // Wait for daemon to be ready (status file signals readiness after sync)
            let status_file = root.join(STATUS_FILE);
            if !wait_for_status_file(&status_file) {
                eprintln!("Daemon not ready (status file not found within timeout)");
                return ExitCode::FAILURE;
            }

            let agent_dir = root.join(AGENTS_DIR).join(&name);

            // Create agent directory first
            if let Err(e) = fs::create_dir_all(&agent_dir) {
                eprintln!("Failed to create agent directory: {e}");
                return ExitCode::FAILURE;
            }

            // Set up watcher on agent directory
            let (_watcher, events_rx) = match create_watcher(&agent_dir) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("Failed to create watcher: {e}");
                    return ExitCode::FAILURE;
                }
            };

            // Verify watcher is receiving events before proceeding
            if let Err(e) = verify_watcher_sync(&agent_dir, &events_rx, Duration::from_secs(5)) {
                eprintln!("Watcher sync failed: {e}");
                return ExitCode::FAILURE;
            }

            let transport = Transport::Directory(agent_dir);
            match wait_and_read_task(&transport, &events_rx, &name) {
                Ok(output) => println!("{output}"),
                Err(e) => {
                    eprintln!("{e}");
                    return ExitCode::FAILURE;
                }
            }
        }
        Command::NextTask {
            pool,
            name,
            data,
            file,
            log_level,
            deregister,
        } => {
            init_tracing(log_level);
            let root = resolve_pool(&pool_root, &pool);
            let agent_dir = root.join(AGENTS_DIR).join(&name);

            if !agent_dir.exists() {
                eprintln!("Agent not registered. Use 'register' first.");
                return ExitCode::FAILURE;
            }

            // Get response content from --data or --file
            let response_content = match (data, file) {
                (Some(d), _) => d,
                (None, Some(path)) => match fs::read_to_string(&path) {
                    Ok(c) => c,
                    Err(e) => {
                        eprintln!("Failed to read response file {}: {e}", path.display());
                        return ExitCode::FAILURE;
                    }
                },
                (None, None) => {
                    eprintln!("Either --data or --file must be provided");
                    return ExitCode::FAILURE;
                }
            };

            // Set up watcher on agent directory
            let (_watcher, events_rx) = match create_watcher(&agent_dir) {
                Ok(w) => w,
                Err(e) => {
                    eprintln!("Failed to create watcher: {e}");
                    return ExitCode::FAILURE;
                }
            };

            // Verify watcher is receiving events before proceeding
            if let Err(e) = verify_watcher_sync(&agent_dir, &events_rx, Duration::from_secs(5)) {
                eprintln!("Watcher sync failed: {e}");
                return ExitCode::FAILURE;
            }

            let transport = Transport::Directory(agent_dir.clone());

            // Write response using Transport (atomic write)
            if let Err(e) = transport.write(RESPONSE_FILE, &response_content) {
                eprintln!("Failed to write response: {e}");
                return ExitCode::FAILURE;
            }

            if deregister {
                // Wait for daemon to acknowledge (response.json deleted)
                let response_path = agent_dir.join(RESPONSE_FILE);
                if !wait_for_response_acknowledged(&response_path, &events_rx) {
                    eprintln!("Timeout waiting for daemon to acknowledge response");
                    return ExitCode::FAILURE;
                }

                // Remove agent directory to deregister
                if let Err(e) = fs::remove_dir_all(&agent_dir) {
                    eprintln!("Failed to deregister: {e}");
                    return ExitCode::FAILURE;
                }
                eprintln!("Deregistered");
            } else {
                // Wait for next task (wait_for_task handles cleanup transition automatically:
                // it blocks until task.json exists AND response.json doesn't)
                match wait_and_read_task(&transport, &events_rx, &name) {
                    Ok(output) => println!("{output}"),
                    Err(e) => {
                        eprintln!("{e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
        }
    }

    ExitCode::SUCCESS
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

/// Wait for the daemon to acknowledge the response (delete response.json).
/// Returns true if acknowledged within timeout, false otherwise.
fn wait_for_response_acknowledged(
    response_path: &std::path::Path,
    events_rx: &mpsc::Receiver<AgentEvent>,
) -> bool {
    const TIMEOUT: Duration = Duration::from_secs(30);

    // Already gone?
    if !response_path.exists() {
        return true;
    }

    let start = std::time::Instant::now();
    loop {
        // Wait for file change event or timeout
        match events_rx.recv_timeout(Duration::from_millis(500)) {
            Ok(AgentEvent::FileChanged) => {
                if !response_path.exists() {
                    return true;
                }
            }
            Ok(AgentEvent::WatchError(_)) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                return false;
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Check timeout
                if start.elapsed() > TIMEOUT {
                    return false;
                }
                // Also check file in case we missed an event
                if !response_path.exists() {
                    return true;
                }
            }
        }
    }
}
