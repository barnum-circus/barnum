//! GSD - Get Shell Scripts Done
//!
//! High-level JSON-based task orchestrator that sits on top of `agent_pool`.
//!
//! # Overview
//!
//! GSD provides a declarative way to define task state machines via JSON config.
//! It validates tasks against JSON schemas, generates documentation for agents,
//! and handles timeouts and retries.
//!
//! # Example Config
//!
//! ```json
//! {
//!   "options": { "timeout": 120 },
//!   "steps": [
//!     {
//!       "name": "Analyze",
//!       "instructions": "Analyze the input.",
//!       "next": ["Implement", "Done"]
//!     },
//!     {
//!       "name": "Implement",
//!       "schema": { "kind": "Inline", "value": { "type": "object" } },
//!       "next": ["Test"]
//!     },
//!     {
//!       "name": "Test",
//!       "next": ["Done", "Implement"]
//!     },
//!     {
//!       "name": "Done",
//!       "next": []
//!     }
//!   ]
//! }
//! ```
//!
//! # Task Format
//!
//! Tasks are JSON objects with `kind` and `value`:
//! ```json
//! {"kind": "Analyze", "value": {"file": "main.rs"}}
//! ```

pub mod config;
pub mod docs;
pub mod runner;
pub mod schema;

pub use config::{Config, ConfigError, Options, SchemaRef, Step};
pub use docs::{generate_full_docs, generate_step_docs};
pub use runner::{RunnerConfig, run};
pub use schema::{CompiledSchemas, ResponseValidationError, Task, ValidationError};
