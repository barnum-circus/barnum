//! GSD Config - Declarative Task Orchestration
//!
//! Config-based task orchestrator that sits on top of `agent_pool`.
//!
//! # Overview
//!
//! Define task workflows via a declarative config. This crate:
//! - Validates tasks against JSON schemas
//! - Generates markdown documentation for agents
//! - Handles timeouts and retries with per-step options
//!
//! The config format is serialization-agnostic (uses serde). The CLI
//! handles parsing from JSON or other formats.
//!
//! # Task Format
//!
//! Tasks have a `kind` (step name) and `value` (payload).
//! Agents return arrays of tasks as their response.

pub mod config;
mod docs;
mod runner;
mod value_schema;

// Public API - only what external users need
pub use config::Config;
pub use docs::generate_full_docs;
pub use runner::{RunnerConfig, TaskOutcome, TaskResult, TaskRunner, run};
pub use value_schema::{CompiledSchemas, Task};
