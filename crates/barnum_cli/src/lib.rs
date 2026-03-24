//! CLI type definitions for Barnum.
//!
//! These types are used by both the CLI binary and the schema generator.
//! They derive `Serialize` and `JsonSchema` so that `emit_zod` can produce
//! a TypeScript Zod schema for programmatic invocation from Node.

use clap::{Parser, Subcommand, ValueEnum};
use schemars::JsonSchema;
use serde::Serialize;
use std::path::PathBuf;

/// Log level for barnum output.
#[derive(Debug, Clone, Copy, Default, ValueEnum, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
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

/// Top-level CLI arguments for barnum.
#[derive(Parser, Serialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
#[command(name = "barnum")]
#[command(about = "Barnum - workflow engine for agents")]
pub struct Cli {
    /// Log level (debug shows task return values)
    #[arg(short, long, global = true, default_value = "info")]
    pub log_level: LogLevel,

    /// Subcommand to run.
    #[command(subcommand)]
    pub command: Command,
}

/// Barnum subcommands.
#[derive(Subcommand, Serialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum Command {
    /// Run the task queue
    #[serde(rename_all = "camelCase")]
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

        /// Executor command for TypeScript handlers, injected by cli.cjs and run.ts.
        #[arg(long, hide = true)]
        executor: String,

        /// Path to run-handler.ts, injected by cli.cjs and run.ts.
        #[arg(long, hide = true)]
        run_handler_path: String,
    },

    /// Config file operations (docs, validate, graph, schema)
    Config {
        /// Config subcommand to run.
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

/// Subcommands for `barnum config`.
#[derive(Subcommand, Serialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum ConfigCommand {
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

    /// Print the config schema (Zod by default, JSON with --type json)
    #[serde(rename_all = "camelCase")]
    Schema {
        /// Output format: zod (default) or json
        #[arg(long = "type", default_value = "zod")]
        schema_type: SchemaType,
    },
}

/// Output format for `barnum config schema`.
#[derive(Debug, Clone, Copy, Default, ValueEnum, Serialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum SchemaType {
    /// Zod TypeScript schema.
    #[default]
    Zod,
    /// JSON Schema.
    Json,
}
