//! GSD CLI - Get Sh*** Done.
//!
//! Command-line interface for the GSD JSON-based task orchestrator.

#![expect(clippy::print_stdout)]
#![expect(clippy::print_stderr)]

use agent_pool_cli::AgentPoolCli;
use clap::{Parser, Subcommand};
use cli_invoker::Invoker;
use gsd_config::{
    Action, CompiledSchemas, Config, RunnerConfig, Task, config_schema, generate_full_docs, run,
};
use std::fs::File;
use std::io;
use std::path::PathBuf;
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

const VERSION: &str = env!("GSD_VERSION");

#[derive(Parser)]
#[command(name = "gsd")]
#[command(about = "Get Sh*** Done - JSON-based task orchestrator")]
struct Cli {
    /// Root directory. Pools live in `<root>/pools/<id>/`.
    /// Defaults to `/tmp/agent_pool` on Unix.
    #[arg(long, global = true)]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Run the task queue
    Run {
        /// Config (JSON string or path to file)
        config: String,

        /// Initial tasks (JSON string or path to file).
        /// Required if config has no `entrypoint`. Cannot be used with `--entrypoint-value`.
        #[arg(long)]
        initial_state: Option<String>,

        /// Initial value for the entrypoint step (JSON string or path to file).
        /// Only valid when config has an `entrypoint`. Defaults to `{}` if not provided.
        #[arg(long)]
        entrypoint_value: Option<String>,

        /// Agent pool ID (e.g., `abc123` resolves to `<root>/pools/abc123/`)
        #[arg(long)]
        pool: Option<String>,

        /// Wake script to call before starting
        #[arg(long)]
        wake: Option<String>,

        /// Log file path (logs emitted in addition to stderr)
        #[arg(long)]
        log_file: Option<PathBuf>,
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
}

#[derive(Subcommand)]
enum ConfigCommand {
    /// Generate markdown documentation from config
    Docs {
        /// Config (JSON string or path to file)
        config: String,
    },

    /// Validate a config file
    Validate {
        /// Config (JSON string or path to file)
        config: String,
    },

    /// Generate DOT visualization of config (for `GraphViz`)
    Graph {
        /// Config (JSON string or path to file)
        config: String,
    },

    /// Print the JSON schema for config files
    Schema,
}

fn main() -> io::Result<()> {
    let cli = Cli::parse();

    // Extract root for commands that need it
    let root = cli.root.unwrap_or_else(agent_pool::default_root);

    match cli.command {
        Command::Run {
            config,
            initial_state,
            entrypoint_value,
            pool,
            wake,
            log_file,
        } => run_command(
            &config,
            initial_state.as_deref(),
            entrypoint_value.as_deref(),
            pool.as_deref(),
            wake.as_deref(),
            log_file.as_ref(),
            &root,
        )?,

        Command::Config { command } => match command {
            ConfigCommand::Docs { config } => {
                let (cfg, config_dir) = parse_config(&config)?;
                let docs = generate_full_docs(&cfg, &config_dir);
                print!("{docs}");
            }

            ConfigCommand::Validate { config } => {
                let (cfg, _) = parse_config(&config)?;
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
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            format!("[E052] config validation failed: {e}"),
                        ));
                    }
                }
            }

            ConfigCommand::Graph { config } => {
                let (cfg, _) = parse_config(&config)?;
                cfg.validate().map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("[E053] config validation failed: {e}"),
                    )
                })?;
                let dot = generate_graphviz(&cfg);
                print!("{dot}");
            }

            ConfigCommand::Schema => {
                let schema = config_schema();
                let json = serde_json::to_string_pretty(&schema).map_err(|e| {
                    io::Error::other(format!("[E059] failed to serialize schema: {e}"))
                })?;
                println!("{json}");
            }
        },

        Command::Version { json } => {
            if json {
                println!(r#"{{"version": "{VERSION}"}}"#);
            } else {
                println!("{VERSION}");
            }
        }
    }

    Ok(())
}

fn run_command(
    config: &str,
    initial_state: Option<&str>,
    entrypoint_value: Option<&str>,
    pool: Option<&str>,
    wake: Option<&str>,
    log_file: Option<&PathBuf>,
    root: &std::path::Path,
) -> io::Result<()> {
    // Initialize tracing with optional log file
    init_tracing(log_file)?;

    // Detect how to invoke the agent_pool CLI (returns helpful error if not found)
    let invoker = Invoker::<AgentPoolCli>::detect()?;

    let (cfg, config_dir) = parse_config(config)?;
    cfg.validate().map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("[E051] config validation failed: {e}"),
        )
    })?;

    let schemas = CompiledSchemas::compile(&cfg, &config_dir)?;

    // Resolve initial tasks based on entrypoint or initial_state
    let initial_tasks = resolve_initial_tasks(&cfg, &schemas, initial_state, entrypoint_value)?;

    // Resolve pool ID
    let pool_path = resolve_pool_path(pool, root)?;

    let runner_config = RunnerConfig {
        agent_pool_root: &pool_path,
        config_base_path: &config_dir,
        wake_script: wake,
        initial_tasks,
        invoker: &invoker,
    };

    run(&cfg, &schemas, runner_config)
}

/// Resolve pool ID to full path.
///
/// Pool IDs cannot contain `/` - use `--root` to specify the base directory.
fn resolve_pool_path(pool: Option<&str>, root: &std::path::Path) -> io::Result<PathBuf> {
    match pool {
        None => {
            // Default pool lives in <root>/pools/default
            let path = agent_pool::pools_dir(root).join("default");
            std::fs::create_dir_all(&path).ok();
            Ok(path)
        }
        Some(p) => {
            if p.contains('/') {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    format!(
                        "[E058] pool ID '{p}' cannot contain '/'. Use --root to specify the base directory."
                    ),
                ));
            }
            Ok(agent_pool::resolve_pool(root, p))
        }
    }
}

/// Parse config from either inline JSON/JSONC or a file path.
/// Returns the config and the directory for resolving relative schema paths.
/// Supports JSONC (JSON with comments) in both cases.
fn parse_config(input: &str) -> io::Result<(Config, PathBuf)> {
    let path = PathBuf::from(input);
    if path.exists() {
        let content = std::fs::read_to_string(&path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!("[E054] failed to read config file {}: {e}", path.display()),
            )
        })?;
        let cfg: Config = json5::from_str(&content).map_err(|e| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("[E055] invalid config in {}: {e}", path.display()),
            )
        })?;
        let dir = path.parent().unwrap_or_else(|| std::path::Path::new("."));
        Ok((cfg, dir.to_path_buf()))
    } else {
        // Assume inline JSON/JSONC
        let cfg: Config = json5::from_str(input).map_err(|e| {
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
    cfg: &Config,
    schemas: &CompiledSchemas,
    initial_state: Option<&str>,
    entrypoint_value: Option<&str>,
) -> io::Result<Vec<Task>> {
    match (&cfg.entrypoint, initial_state, entrypoint_value) {
        // Config has entrypoint
        (Some(entrypoint), None, ev) => {
            // Parse entrypoint value (default to empty object)
            let value: serde_json::Value = match ev {
                Some(v) => parse_json_input(v).map_err(|e| {
                    io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("[E060] invalid --entrypoint-value JSON: {e}"),
                    )
                })?,
                None => serde_json::json!({}),
            };

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

fn init_tracing(log_file: Option<&PathBuf>) -> io::Result<()> {
    let filter =
        EnvFilter::from_default_env().add_directive("gsd=info".parse().unwrap_or_default());

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
        "digraph GSD {".to_string(),
        "  rankdir=TB;".to_string(),
        "  node [fontname=\"Helvetica\"];".to_string(),
        "  edge [fontname=\"Helvetica\", fontsize=10];".to_string(),
        String::new(),
    ];

    // Define nodes with attributes based on step type
    for step in &config.steps {
        let mut attrs: Vec<String> = vec![];

        // Shape based on action type
        match &step.action {
            Action::Pool { .. } => attrs.push("shape=box".to_string()),
            Action::Command { .. } => attrs.push("shape=diamond".to_string()),
        }

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

        // Color based on action type
        match &step.action {
            Action::Pool { .. } => {
                attrs.push("style=filled, fillcolor=\"#e3f2fd\"".to_string());
            }
            Action::Command { .. } => {
                attrs.push("style=filled, fillcolor=\"#fff3e0\"".to_string());
            }
        }

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

    #[test]
    fn graphviz_basic() {
        let config: Config = serde_json::from_str(
            r#"{
                "steps": [
                    {"name": "Start", "next": ["Middle"]},
                    {"name": "Middle", "next": ["End"]},
                    {"name": "End", "next": []}
                ]
            }"#,
        )
        .unwrap();

        let dot = generate_graphviz(&config);
        assert!(dot.contains("digraph GSD"));
        assert!(dot.contains("\"Start\" -> \"Middle\""));
        assert!(dot.contains("\"Middle\" -> \"End\""));
        assert!(dot.contains("peripheries=2")); // End is terminal
    }

    // =========================================================================
    // resolve_initial_tasks tests
    // =========================================================================

    fn make_config(entrypoint: Option<&str>) -> Config {
        let mut cfg: Config =
            serde_json::from_str(r#"{"steps": [{"name": "Start", "next": []}]}"#).unwrap();
        cfg.entrypoint = entrypoint.map(|s| s.to_string().into());
        cfg
    }

    fn make_schemas(cfg: &Config) -> CompiledSchemas {
        CompiledSchemas::compile(cfg, std::path::Path::new(".")).unwrap()
    }

    #[test]
    fn resolve_with_entrypoint_and_no_flags() {
        // Config has entrypoint, no flags provided -> uses {} as value
        let cfg = make_config(Some("Start"));
        let schemas = make_schemas(&cfg);

        let result = resolve_initial_tasks(&cfg, &schemas, None, None);
        assert!(result.is_ok());
        let tasks = result.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].step.as_str(), "Start");
    }

    #[test]
    fn resolve_with_entrypoint_and_entrypoint_value() {
        // Config has entrypoint, --entrypoint-value provided
        let cfg = make_config(Some("Start"));
        let schemas = make_schemas(&cfg);

        let result = resolve_initial_tasks(&cfg, &schemas, None, Some(r#"{"foo": 1}"#));
        assert!(result.is_ok());
        let tasks = result.unwrap();
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].step.as_str(), "Start");
    }

    #[test]
    fn resolve_with_entrypoint_and_initial_state_uses_initial_state() {
        // Config has entrypoint but --initial-state provided -> initial-state takes precedence
        let cfg = make_config(Some("Start"));
        let schemas = make_schemas(&cfg);

        let result = resolve_initial_tasks(
            &cfg,
            &schemas,
            Some(r#"[{"kind": "Start", "value": {}}]"#),
            None,
        );
        assert!(result.is_ok());
        let tasks = result.unwrap();
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn resolve_without_entrypoint_and_initial_state() {
        // No entrypoint, --initial-state provided -> works
        let cfg = make_config(None);
        let schemas = make_schemas(&cfg);

        let result = resolve_initial_tasks(
            &cfg,
            &schemas,
            Some(r#"[{"kind": "Start", "value": {}}]"#),
            None,
        );
        assert!(result.is_ok());
        let tasks = result.unwrap();
        assert_eq!(tasks.len(), 1);
    }

    #[test]
    fn resolve_without_entrypoint_and_entrypoint_value_errors_e063() {
        // No entrypoint but --entrypoint-value provided -> error
        let cfg = make_config(None);
        let schemas = make_schemas(&cfg);

        let result = resolve_initial_tasks(&cfg, &schemas, None, Some(r"{}"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("E063"));
    }

    #[test]
    fn resolve_without_entrypoint_and_no_flags_errors_e064() {
        // No entrypoint, no flags -> error
        let cfg = make_config(None);
        let schemas = make_schemas(&cfg);

        let result = resolve_initial_tasks(&cfg, &schemas, None, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("E064"));
    }

    #[test]
    fn resolve_with_invalid_entrypoint_value_json_errors_e060() {
        // Config has entrypoint, invalid JSON in --entrypoint-value
        let cfg = make_config(Some("Start"));
        let schemas = make_schemas(&cfg);

        let result = resolve_initial_tasks(&cfg, &schemas, None, Some("not json"));
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("E060"));
    }

    #[test]
    fn resolve_validates_entrypoint_value_against_schema_e061() {
        // Config has entrypoint with schema, value doesn't match -> error
        let mut cfg: Config = serde_json::from_str(
            r#"{
                "steps": [{
                    "name": "Start",
                    "value_schema": {
                        "type": "object",
                        "required": ["path"],
                        "properties": {"path": {"type": "string"}}
                    },
                    "next": []
                }]
            }"#,
        )
        .unwrap();
        cfg.entrypoint = Some("Start".to_string().into());
        let schemas = make_schemas(&cfg);

        // Empty object doesn't satisfy required "path"
        let result = resolve_initial_tasks(&cfg, &schemas, None, None);
        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(err.to_string().contains("E061"));
    }

    #[test]
    fn resolve_allows_empty_value_when_no_schema() {
        // Config has entrypoint without schema -> {} is allowed
        let cfg = make_config(Some("Start"));
        let schemas = make_schemas(&cfg);

        let result = resolve_initial_tasks(&cfg, &schemas, None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn resolve_allows_empty_value_when_schema_is_empty_object() {
        // Config has entrypoint with schema that accepts empty object
        let mut cfg: Config = serde_json::from_str(
            r#"{
                "steps": [{
                    "name": "Start",
                    "value_schema": {"type": "object"},
                    "next": []
                }]
            }"#,
        )
        .unwrap();
        cfg.entrypoint = Some("Start".to_string().into());
        let schemas = make_schemas(&cfg);

        let result = resolve_initial_tasks(&cfg, &schemas, None, None);
        assert!(result.is_ok());
    }
}
