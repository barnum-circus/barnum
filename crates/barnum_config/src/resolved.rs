//! Fully resolved configuration types.
//!
//! These types have all file references resolved and options computed.
//! They're the runtime representation after loading a config file.

use crate::types::{HookScript, StepName};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Fully resolved Barnum configuration.
///
/// All file references have been resolved and options computed per-step.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Config {
    /// Maximum concurrent tasks (None = use default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_concurrency: Option<usize>,
    /// Resolved step definitions.
    pub steps: Vec<Step>,
}

impl Config {
    /// Build a map of step name to step for efficient lookup.
    #[must_use]
    pub fn step_map(&self) -> HashMap<&StepName, &Step> {
        self.steps.iter().map(|s| (&s.name, s)).collect()
    }
}

/// A fully resolved step.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// Step name.
    pub name: StepName,

    /// How to execute the step.
    pub action: ActionKind,

    /// Valid next steps.
    #[serde(default)]
    pub next: Vec<StepName>,

    /// Finally hook (runs after all children complete).
    #[serde(default, rename = "finally", skip_serializing_if = "Option::is_none")]
    pub finally_hook: Option<HookScript>,

    /// Effective options (global + per-step merged).
    pub options: Options,
}

/// Run a shell command.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct BashAction {
    /// Shell script to execute.
    pub script: String,
}

/// How a resolved step processes tasks.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum ActionKind {
    /// Run a shell command.
    Bash(BashAction),
}

/// Resolved options for a step.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Options {
    /// Timeout in seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout: Option<u64>,
    /// Maximum retries.
    #[serde(default)]
    pub max_retries: u32,
    /// Whether to retry on timeout.
    #[serde(default = "default_true")]
    pub retry_on_timeout: bool,
    /// Whether to retry on invalid response.
    #[serde(default = "default_true")]
    pub retry_on_invalid_response: bool,
}

const fn default_true() -> bool {
    true
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout: None,
            max_retries: 0,
            retry_on_timeout: true,
            retry_on_invalid_response: true,
        }
    }
}

/// Root type for generating the resolved schema.
///
/// Groups the resolved config and task types so `schema_for!` produces
/// a single schema containing all resolved runtime types. This struct
/// exists only for schema generation — it's never constructed at runtime.
#[derive(Debug, JsonSchema)]
#[expect(dead_code)]
pub struct ResolvedTypes {
    /// The resolved configuration.
    config: Config,
    /// A task (agent response element).
    task: crate::types::Task,
}

/// Generate the schemars `RootSchema` for all resolved runtime types.
///
/// This feeds into `emit_zod` to produce TypeScript types for resolved configs and tasks.
#[must_use]
pub fn resolved_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(ResolvedTypes)
}
