//! Fully resolved configuration types.
//!
//! These types have all file references resolved and options computed.
//! They're the runtime representation after loading a config file.

use crate::types::StepName;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Fully resolved GSD configuration.
///
/// All file references have been resolved and options computed per-step.
#[derive(Debug, Serialize, Deserialize)]
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

    /// Check if any step uses the Pool action.
    #[must_use]
    pub fn has_pool_actions(&self) -> bool {
        self.steps
            .iter()
            .any(|s| matches!(s.action, Action::Pool { .. }))
    }
}

/// A fully resolved step.
#[derive(Debug, Serialize, Deserialize)]
pub struct Step {
    /// Step name.
    pub name: StepName,

    /// Resolved JSON Schema for validating the step's value payload.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_schema: Option<serde_json::Value>,

    /// Shell command to run before the action.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pre: Option<String>,

    /// How this step processes tasks.
    pub action: Action,

    /// Shell command to run after the action completes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post: Option<String>,

    /// Valid next step names (empty = terminal step).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub next: Vec<StepName>,

    /// Shell command to run after all descendants complete.
    #[serde(default, rename = "finally", skip_serializing_if = "Option::is_none")]
    pub finally_hook: Option<String>,

    /// Effective options (global + per-step merged).
    pub options: Options,
}

/// How a resolved step processes tasks.
#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Action {
    /// Send to the agent pool for processing.
    Pool {
        /// Resolved markdown instructions.
        instructions: String,
    },
    /// Run a local command.
    Command {
        /// Shell script to execute.
        script: String,
    },
}

/// Resolved options for a step.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
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
