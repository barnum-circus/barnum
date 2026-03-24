//! Barnum Config - Declarative Task Orchestration
//!
//! Config-based task orchestrator that sits on top of `troupe`.
//!
//! # Overview
//!
//! Define task workflows via a declarative config. This crate:
//! - Generates markdown documentation for agents
//! - Validates transitions between steps
//! - Handles timeouts and retries with per-step options
//!
//! The config format is serialization-agnostic (uses serde). The CLI
//! handles parsing from JSON or other formats.
//!
//! # Task Format
//!
//! Tasks have a `kind` (step name) and `value` (payload).
//! Agents return arrays of tasks as their response.

mod config;
mod docs;
mod runner;
mod types;
pub mod zod;

// Public API - only what barnum_cli actually uses
pub use config::{ActionKind, Config, EffectiveOptions, config_schema};
pub use docs::generate_full_docs;
pub use runner::{RunnerConfig, resume, run};
pub use types::{StepInputValue, StepName, Task};
