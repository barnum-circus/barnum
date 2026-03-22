//! Barnum CLI - workflow engine for agents.
//!
//! Command-line interface for Barnum.

#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use barnum_config::{
    Action, CompiledSchemas, Config, ConfigFile, RunnerConfig, StepInputValue, Task, config_schema,
    generate_full_docs, resume, run,
};
use clap::{Parser, Subcommand, ValueEnum};
use cli_invoker::Invoker;
use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};
use troupe_cli::TroupeCli;

/// Log level for barnum output.
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
    /// Debug messages (includes task return values)
    Debug,
    /// Trace messages (very verbose)
    Trace,
}

const VERSION: &str = env!("BARNUM_VERSION");

#[derive(Parser)]
#[command(name = "barnum")]
#[command(about = "Barnum - workflow engine for agents")]
struct Cli {
    /// Root directory. Pools live in `<root>/pools/<id>/`.
    /// Defaults to `/tmp/troupe` on Unix.
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    /// Log level (debug shows task return values)
    #[arg(short, long, global = true, default_value = "info")]
    log_level: LogLevel,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the task queue
    Run {
        /// Config (JSON string or path to file). Required unless `--resume-from` is used.
        #[arg(long, required_unless_present = "resume_from")]
        config: Option<String>,

        /// Initial tasks (JSON string or path to file).
        /// Required if config has no `entrypoint`. Cannot be used with `--entrypoint-value`.
        #[arg(long, conflicts_with = "resume_from")]
        initial_state: Option<String>,

        /// Initial value for the entrypoint step (JSON string or path to file).
        /// Only valid when config has an `entrypoint`. Defaults to `{}` if not provided.
        #[arg(long, conflicts_with = "resume_from")]
        entrypoint_value: Option<String>,

        /// Agent pool ID (e.g., `abc123` resolves to `<root>/pools/abc123/`).
        /// Defaults to `default`.
        #[arg(long)]
        pool: Option<String>,

        /// Wake script to call before starting
        #[arg(long)]
        wake: Option<String>,

        /// Log file path (logs emitted in addition to stderr)
        #[arg(long)]
        log_file: Option<PathBuf>,

        /// State log file path (NDJSON file for persistence/resume)
        #[arg(long)]
        state_log: Option<PathBuf>,

        /// Resume from a previous state log file.
        /// Incompatible with `--config`, `--initial-state`, and `--entrypoint-value`.
        #[arg(long, conflicts_with = "config")]
        resume_from: Option<PathBuf>,
    },

    /// Config file operations (docs, validate, graph, schema)
    Config {
        #[command(subcommand)]
        command: ConfigCommand,
    },

    /// Print version information
    Version {
        /// Output as JSON (for programmatic access)
        #[arg(long)]
        json: bool,
    },

    /// Launch the TUI dashboard (requires barnum-tui binary)
    Tui {
        /// Arguments passed through to barnum-tui
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
    },
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Generate markdown documentation from config
    Docs {
        /// Config (JSON string or path to file)
        #[arg(long)]
        config: String,
    },

    /// Validate a config file
    Validate {
        /// Config (JSON string or path to file)
        #[arg(long)]
        config: String,
    },

    /// Generate DOT visualization of config (for `GraphViz`)
    Graph {
        /// Config (JSON string or path to file)
        #[arg(long)]
        config: String,
    },

    /// Print the JSON schema for config files
    Schema,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    // Extract root for commands that need it
    let root = cli.root.unwrap_or_else(troupe::default_root);
    let log_level = cli.log_level;

    match cli.command {
        Command::Run {
            config,
            initial_state,
            entrypoint_value,
            pool,
            wake,
            log_file,
            state_log,
            resume_from,
        } => match (resume_from, config) {
            (Some(resume_path), _) => resume_command(
                &resume_path,
                pool.as_deref(),
                wake.as_deref(),
                log_file.as_ref(),
                state_log.as_ref(),
                &root,
                log_level,
            )?,
            (None, Some(config)) => run_command(
                &config,
                initial_state.as_deref(),
                entrypoint_value.as_deref(),
                pool.as_deref(),
                wake.as_deref(),
                log_file.as_ref(),
                state_log.as_ref(),
                &root,
                log_level,
            )?,
            (None, None) => {
                // Unreachable: clap's required_unless_present prevents this
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "[E073] --config is required when not using --resume-from",
                ));
            }
        },

        Command::Config { command } => handle_config_command(command)?,

        Command::Version { json } => {
            if json {
                println!(r#"{{"version": "{VERSION}"}}"#);
            } else {
                println!("{VERSION}");
            }
        }

        Command::Tui { args } => {
            let status = std::process::Command::new("barnum-tui")
                .args(&args)
                .status()
                .map_err(|e| {
                    io::Error::new(
                        e.kind(),
                        format!(
                            "Failed to run barnum-tui: {e}. Is it installed? \
                             Try: cargo install --path crates/barnum_tui"
                        ),
                    )
                })?;
            std::process::exit(status.code().unwrap_or(1));
        }
    }

    Ok(())
}

fn handle_config_command(command: ConfigCommand) -> io::Result<()> {
    match command {
        ConfigCommand::Docs { config } => {
            let (config_file, config_dir) = parse_config(&config)?;
            let cfg = config_file.resolve(&config_dir)?;
            let docs = generate_full_docs(&cfg);
            print!("{docs}");
        }

        ConfigCommand::Validate { config } => {
            let (config_file, config_dir) = parse_config(&config)?;
            match config_file.validate() {
                Ok(()) => {
                    let cfg = config_file.resolve(&config_dir)?;
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
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("[E052] config validation failed: {e}"),
                    ));
                }
            }
        }

        ConfigCommand::Graph { config } => {
            let (config_file, config_dir) = parse_config(&config)?;
            config_file.validate().map_err(|e| {
                io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("[E053] config validation failed: {e}"),
                )
            })?;
            let cfg = config_file.resolve(&config_dir)?;
            let dot = generate_graphviz(&cfg);
            print!("{dot}");
        }

        ConfigCommand::Schema => {
            let schema = config_schema();
            let json = serde_json::to_string_pretty(&schema)
                .map_err(|e| io::Error::other(format!("[E059] failed to serialize schema: {e}")))?;
            println!("{json}");
        }
    }
    Ok(())
}

#[expect(clippy::too_many_arguments)]
fn run_command(
    config: &str,
    initial_state: Option<&str>,
    entrypoint_value: Option<&str>,
    pool: Option<&str>,
    wake: Option<&str>,
    log_file: Option<&PathBuf>,
    state_log: Option<&PathBuf>,
    root: &std::path::Path,
    log_level: LogLevel,
) -> io::Result<()> {
    // Initialize tracing with optional log file
    init_tracing(log_file, log_level)?;

    // Detect how to invoke the troupe CLI (returns helpful error if not found).
    // Pin to our version so dlx fetches the matching troupe release.
    let invoker = Invoker::<TroupeCli>::detect(Some(VERSION))?;

    let (config_file, config_dir) = parse_config(config)?;
    config_file.validate().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("[E051] config validation failed: {e}"),
        )
    })?;

    // Extract entrypoint before resolve consumes config_file
    let entrypoint = config_file.entrypoint.clone();

    // Resolve to runtime config (loads linked files, computes effective options)
    let cfg = config_file.resolve(&config_dir)?;
    let schemas = CompiledSchemas::compile(&cfg)?;

    // Resolve initial tasks based on entrypoint or initial_state
    let initial_tasks = resolve_initial_tasks(
        &schemas,
        initial_state,
        entrypoint_value,
        entrypoint.as_ref(),
    )?;

    // Resolve pool ID
    let pool_id = pool.unwrap_or(troupe::DEFAULT_POOL_ID);
    let pool_path = resolve_pool_path(pool_id, root)?;

    // State log: use explicit path or generate default under <root>/logs/
    let state_log_path = match state_log {
        Some(p) => p.clone(),
        None => default_state_log_path(root, pool_id)?,
    };

    let runner_config = RunnerConfig {
        troupe_root: &pool_path,
        working_dir: &config_dir,
        wake_script: wake,
        invoker: &invoker,
        state_log_path: &state_log_path,
    };

    run(&cfg, &schemas, &runner_config, initial_tasks)
}

fn resume_command(
    resume_from: &Path,
    pool: Option<&str>,
    wake: Option<&str>,
    log_file: Option<&PathBuf>,
    state_log: Option<&PathBuf>,
    root: &std::path::Path,
    log_level: LogLevel,
) -> io::Result<()> {
    init_tracing(log_file, log_level)?;

    let invoker = Invoker::<TroupeCli>::detect(Some(VERSION))?;

    let pool_id = pool.unwrap_or(troupe::DEFAULT_POOL_ID);
    let pool_path = resolve_pool_path(pool_id, root)?;

    // State log: use explicit path or generate default under <root>/logs/
    let state_log_path = match state_log {
        Some(p) => p.clone(),
        None => default_state_log_path(root, pool_id)?,
    };

    // Validate: resume-from and state-log must not be the same path
    let resume_canonical = std::fs::canonicalize(resume_from)?;
    if state_log_path.exists() {
        let state_canonical = std::fs::canonicalize(&state_log_path)?;
        if resume_canonical == state_canonical {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "[E072] --resume-from and --state-log must be different files",
            ));
        }
    }

    // For resume, working_dir defaults to current directory
    // (the original working dir is not stored in the log)
    let working_dir = std::env::current_dir()?;

    let runner_config = RunnerConfig {
        troupe_root: &pool_path,
        working_dir: &working_dir,
        wake_script: wake,
        invoker: &invoker,
        state_log_path: &state_log_path,
    };

    resume(resume_from, &runner_config)
}

/// Resolve pool ID to full path.
///
/// Pool IDs cannot contain `/` - use `--root` to specify the base directory.
fn resolve_pool_path(pool_id: &str, root: &std::path::Path) -> io::Result<PathBuf> {
    if pool_id.contains('/') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "[E058] pool ID '{pool_id}' cannot contain '/'. Use --root to specify the base directory."
            ),
        ));
    }
    Ok(troupe::resolve_pool(root, pool_id))
}

/// Generate a default state log path under `<root>/logs/<pool_id>.<run_id>.ndjson`.
///
/// Creates the `<root>/logs/` directory if it doesn't exist.
fn default_state_log_path(root: &Path, pool_id: &str) -> io::Result<PathBuf> {
    let logs_dir = root.join("logs");
    std::fs::create_dir_all(&logs_dir)?;
    let run_id = troupe::generate_id();
    Ok(logs_dir.join(format!("{pool_id}.{run_id}.ndjson")))
}

/// Parse config from either inline JSON/JSONC or a file path.
/// Returns the config file and the directory for resolving relative paths.
/// Supports JSONC (JSON with comments) in both cases.
fn parse_config(input: &str) -> io::Result<(ConfigFile, PathBuf)> {
    let path = PathBuf::from(input);
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("[E054] failed to read config file {}: {e}", path.display()),
            )
        })?;
        let cfg: ConfigFile = json5::from_str(&content).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("[E055] invalid config in {}: {e}", path.display()),
            )
        })?;
        let canonical = path.canonicalize().map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "[E054] failed to resolve config path {}: {e}",
                    path.display()
                ),
            )
        })?;
        let dir = canonical
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        Ok((cfg, dir.to_path_buf()))
    } else {
        // Assume inline JSON/JSONC
        let cfg: ConfigFile = json5::from_str(input).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("[E056] invalid inline config: {e}"),
            )
        })?;
        Ok((cfg, PathBuf::from(".")))
    }
}

/// Resolve initial tasks from either --initial-state or entrypoint + --entrypoint-value.
fn resolve_initial_tasks(
    schemas: &CompiledSchemas,
    initial_state: Option<&str>,
    entrypoint_value: Option<&str>,
    entrypoint: Option<&barnum_config::StepName>,
) -> io::Result<Vec<Task>> {
    match (entrypoint, initial_state, entrypoint_value) {
        // Config has entrypoint
        (Some(entrypoint), None, ev) => {
            // Parse entrypoint value (default to empty object)
            let value = StepInputValue(match ev {
                Some(v) => parse_json_input(v).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("[E060] invalid --entrypoint-value JSON: {e}"),
                    )
                })?,
                None => serde_json::json!({}),
            });

            // Validate the value against the entrypoint step's schema
            if let Err(e) = schemas.validate(entrypoint, &value) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("[E061] entrypoint value validation failed: {e}"),
                ));
            }

            Ok(vec![Task::new(entrypoint.clone(), value)])
        }

        // --initial-state takes precedence over entrypoint (if present)
        (Some(_), Some(initial), _) | (None, Some(initial), None) => parse_initial_tasks(initial),

        // No entrypoint but --entrypoint-value provided
        (None, _, Some(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "[E063] --entrypoint-value requires config to have an entrypoint",
        )),

        // No entrypoint and no --initial-state
        (None, None, None) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "[E064] --initial-state is required when config has no entrypoint",
        )),
    }
}

/// Parse JSON from a string or file path.
fn parse_json_input(input: &str) -> Result<serde_json::Value, json5::Error> {
    let path = PathBuf::from(input);
    let content = if path.exists() {
        std::fs::read_to_string(path).unwrap_or_else(|_| input.to_string())
    } else {
        input.to_string()
    };
    json5::from_str(&content)
}

fn parse_initial_tasks(initial: &str) -> io::Result<Vec<Task>> {
    // Check if it's a file path
    let content = {
        let path = PathBuf::from(initial);
        if path.exists() {
            std::fs::read_to_string(path)?
        } else {
            // Assume it's inline JSON/JSONC
            initial.to_string()
        }
    };

    json5::from_str(&content).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("[E057] invalid initial tasks JSON: {e}"),
        )
    })
}

fn init_tracing(log_file: Option<&PathBuf>, log_level: LogLevel) -> io::Result<()> {
    let directive = match log_level {
        LogLevel::Off => "barnum=off",
        LogLevel::Error => "barnum=error",
        LogLevel::Warn => "barnum=warn",
        LogLevel::Info => "barnum=info",
        LogLevel::Debug => "barnum=debug",
        LogLevel::Trace => "barnum=trace",
    };
    let filter = EnvFilter::from_default_env().add_directive(directive.parse().unwrap_or_default());

    let stderr_layer = fmt::layer().with_target(false);

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

/// Generate DOT format visualization of the config (for `GraphViz`).
fn generate_graphviz(config: &Config) -> String {
    let mut lines = vec![
        "digraph Barnum {".to_string(),
        "  rankdir=TB;".to_string(),
        "  node [fontname=\"Helvetica\"];".to_string(),
        "  edge [fontname=\"Helvetica\", fontsize=10];".to_string(),
        String::new(),
    ];

    // Define nodes with attributes based on step type
    for step in &config.steps {
        let mut attrs: Vec<String> = vec![];

        // Shape and color based on action type
        let (shape, fill_color) = match &step.action {
            Action::Pool { .. } => ("box", "#e3f2fd"),
            Action::Command { .. } => ("diamond", "#fff3e0"),
        };
        attrs.push(format!("shape={shape}"));

        // Terminal steps get double border
        if step.next.is_empty() {
            attrs.push("peripheries=2".to_string());
        }

        // Build label with hooks indicator
        let mut label_parts = vec![step.name.to_string()];
        let mut hooks = vec![];
        if step.pre.is_some() {
            hooks.push("pre");
        }
        if step.post.is_some() {
            hooks.push("post");
        }
        if step.finally_hook.is_some() {
            hooks.push("finally");
        }
        if !hooks.is_empty() {
            label_parts.push(format!("[{}]", hooks.join(", ")));
        }

        let label = label_parts.join("\\n");
        attrs.push(format!("label=\"{label}\""));
        attrs.push(format!("style=filled, fillcolor=\"{fill_color}\""));

        lines.push(format!("  \"{}\" [{}];", step.name, attrs.join(", ")));
    }

    lines.push(String::new());

    // Define edges
    for step in &config.steps {
        for next in &step.next {
            lines.push(format!("  \"{}\" -> \"{next}\";", step.name));
        }
    }

    lines.push("}".to_string());
    lines.join("\n")
}

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::path::Path;

    fn resolve_config(json: &str) -> Config {
        let config_file: ConfigFile = serde_json::from_str(json).unwrap();
        config_file.resolve(Path::new(".")).unwrap()
    }

    const POOL: &str = r#"{"kind": "Pool", "instructions": {"inline": ""}}"#;

    #[test]
    fn graphviz_basic() {
        let json = format!(
            r#"{{
                "steps": [
                    {{"name": "Start", "action": {POOL}, "next": ["Middle"]}},
                    {{"name": "Middle", "action": {POOL}, "next": ["End"]}},
                    {{"name": "End", "action": {POOL}, "next": []}}
                ]
            }}"#
        );
        let config = resolve_config(&json);

        let dot = generate_graphviz(&config);
        assert!(dot.contains("digraph Barnum"));
        assert!(dot.contains("\"Start\" -> \"Middle\""));
        assert!(dot.contains("\"Middle\" -> \"End\""));
        assert!(dot.contains("peripheries=2")); // End is terminal
    }

    // =========================================================================
    // resolve_initial_tasks tests
    // =========================================================================

    fn make_config_and_schemas(
        json: &str,
        entrypoint: Option<&str>,
    ) -> (Config, CompiledSchemas, Option<barnum_config::StepName>) {
        let mut config_file: ConfigFile = serde_json::from_str(json).unwrap();
        config_file.entrypoint = entrypoint.map(|s| s.to_string().into());
        let ep = config_file.entrypoint.clone();
        let cfg = config_file.resolve(Path::new(".")).unwrap();
        let schemas = CompiledSchemas::compile(&cfg).unwrap();
        (cfg, schemas, ep)
    }

    fn simple_config() -> String {
        format!(r#"{{"steps": [{{"name": "Start", "action": {POOL}, "next": []}}]}}"#)
    }

    #[test]
    fn resolve_with_entrypoint_and_no_flags() {
        // Config has entrypoint, no flags provided -> uses {} as value
        let (_cfg, schemas, ep) = make_config_and_schemas(&simple_config(), Some("Start"));

        let result = resolve_initial_tasks(&schemas, None, None, ep.as_ref());
        assert!(result.is_ok());
        let tasks = result.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].step.as_str(), "Start");
    }

    #[test]
    fn resolve_with_entrypoint_and_entrypoint_value() {
        // Config has entrypoint, --entrypoint-value provided
        let (_cfg, schemas, ep) = make_config_and_schemas(&simple_config(), Some("Start"));

        let result = resolve_initial_tasks(&schemas, None, Some(r#"{"foo": 1}"#), ep.as_ref());
        assert!(result.is_ok());
        let tasks = result.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].step.as_str(), "Start");
    }

    #[test]
    fn resolve_with_entrypoint_and_initial_state_uses_initial_state() {
        // Config has entrypoint but --initial-state provided -> initial-state takes precedence
        let (_cfg, schemas, ep) = make_config_and_schemas(&simple_config(), Some("Start"));

        let result = resolve_initial_tasks(
            &schemas,
            Some(r#"[{"kind": "Start", "value": {}}]"#),
            None,
            ep.as_ref(),
        );
        assert!(result.is_ok());
        let tasks = result.unwrap();
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn resolve_without_entrypoint_and_initial_state() {
        // No entrypoint, --initial-state provided -> works
        let (_cfg, schemas, ep) = make_config_and_schemas(&simple_config(), None);

        let result = resolve_initial_tasks(
            &schemas,
            Some(r#"[{"kind": "Start", "value": {}}]"#),
            None,
            ep.as_ref(),
        );
        assert!(result.is_ok());
        let tasks = result.unwrap();
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn resolve_without_entrypoint_and_entrypoint_value_errors_e063() {
        // No entrypoint but --entrypoint-value provided -> error
        let (_cfg, schemas, ep) = make_config_and_schemas(&simple_config(), None);

        let result = resolve_initial_tasks(&schemas, None, Some(r"{}"), ep.as_ref());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("E063"));
    }

    #[test]
    fn resolve_without_entrypoint_and_no_flags_errors_e064() {
        // No entrypoint, no flags -> error
        let (_cfg, schemas, ep) = make_config_and_schemas(&simple_config(), None);

        let result = resolve_initial_tasks(&schemas, None, None, ep.as_ref());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("E064"));
    }

    #[test]
    fn resolve_with_invalid_entrypoint_value_json_errors_e060() {
        // Config has entrypoint, invalid JSON in --entrypoint-value
        let (_cfg, schemas, ep) = make_config_and_schemas(&simple_config(), Some("Start"));

        let result = resolve_initial_tasks(&schemas, None, Some("not json"), ep.as_ref());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("E060"));
    }

    #[test]
    fn resolve_validates_entrypoint_value_against_schema_e061() {
        // Config has entrypoint with schema, value doesn't match -> error
        let config_with_schema = format!(
            r#"{{
            "steps": [{{
                "name": "Start",
                "action": {POOL},
                "value_schema": {{
                    "type": "object",
                    "required": ["path"],
                    "properties": {{"path": {{"type": "string"}}}}
                }},
                "next": []
            }}]
        }}"#
        );
        let (_cfg, schemas, ep) = make_config_and_schemas(&config_with_schema, Some("Start"));

        // Empty object doesn't satisfy required "path"
        let result = resolve_initial_tasks(&schemas, None, None, ep.as_ref());
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("E061"));
    }

    #[test]
    fn resolve_allows_empty_value_when_no_schema() {
        // Config has entrypoint without schema -> {} is allowed
        let (_cfg, schemas, ep) = make_config_and_schemas(&simple_config(), Some("Start"));

        let result = resolve_initial_tasks(&schemas, None, None, ep.as_ref());
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_allows_empty_value_when_schema_is_empty_object() {
        // Config has entrypoint with schema that accepts empty object
        let config_with_empty_schema = format!(
            r#"{{
            "steps": [{{
                "name": "Start",
                "action": {POOL},
                "value_schema": {{"type": "object"}},
                "next": []
            }}]
        }}"#
        );
        let (_cfg, schemas, ep) = make_config_and_schemas(&config_with_empty_schema, Some("Start"));

        let result = resolve_initial_tasks(&schemas, None, None, ep.as_ref());
        assert!(result.is_ok());
    }
}
