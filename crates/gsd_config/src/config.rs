//! Configuration types for GSD.
//!
//! Defines the task queue with steps, schemas, and transitions.
//! These types are serialization-format agnostic (use serde).

use crate::types::StepName;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level GSD configuration.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// JSON Schema reference for editor validation (ignored at runtime).
    #[serde(rename = "$schema", default, skip_serializing)]
    pub schema_ref: Option<String>,

    /// Runtime options.
    #[serde(default)]
    pub options: Options,

    /// Entry point step name. If specified, the workflow starts with this step
    /// and `--entrypoint-value` can be used to provide the initial value (defaults to `{}`).
    /// If not specified, `--initial-state` must be provided on the command line.
    #[serde(default)]
    pub entrypoint: Option<StepName>,

    /// Step definitions forming the task queue.
    pub steps: Vec<Step>,
}

/// Runtime options for task execution.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Options {
    /// Timeout in seconds for each task (None = no timeout).
    #[serde(default)]
    pub timeout: Option<u64>,

    /// Maximum retries per task (default: 0).
    #[serde(default)]
    pub max_retries: u32,

    /// Maximum concurrent tasks (None = unlimited).
    #[serde(default)]
    pub max_concurrency: Option<usize>,

    /// Whether to retry when agent times out (default: true).
    #[serde(default = "default_true")]
    pub retry_on_timeout: bool,

    /// Whether to retry when agent returns invalid response (default: true).
    #[serde(default = "default_true")]
    pub retry_on_invalid_response: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            timeout: None,
            max_retries: 0,
            max_concurrency: None,
            retry_on_timeout: true,
            retry_on_invalid_response: true,
        }
    }
}

const fn default_true() -> bool {
    true
}

/// A step in the task queue.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Step name (e.g., `Analyze`, `Implement`).
    pub name: StepName,

    /// JSON Schema for validating the step's value payload.
    /// None means any JSON value is accepted.
    #[serde(default)]
    pub value_schema: Option<SchemaRef>,

    /// Shell command to run before the action.
    ///
    /// Receives the task value JSON on stdin, must output modified task value JSON on stdout.
    /// If the command fails (non-zero exit), the task is treated as failed and the post
    /// hook (if any) is called with `{kind: "PreHookError", ...}`.
    #[serde(default)]
    pub pre: Option<String>,

    /// How this step processes tasks.
    #[serde(default)]
    pub action: Action,

    /// Shell command to run after the action completes.
    ///
    /// Receives result JSON on stdin with structure:
    /// - `{kind: "Success", input: <value>, output: <agent_output>, next: [...]}`
    /// - `{kind: "Timeout", input: <value>}`
    /// - `{kind: "Error", input: <value>, error: "..."}`
    /// - `{kind: "PreHookError", input: <value>, error: "..."}`
    ///
    /// Must output modified result JSON on stdout. Can modify the `next` array to
    /// filter, add, or transform the tasks that will be spawned.
    ///
    /// Runs even on timeout or error. Post hook failures trigger retry policy.
    #[serde(default)]
    pub post: Option<String>,

    /// Valid next step names (empty = terminal step).
    #[serde(default)]
    pub next: Vec<StepName>,

    /// Shell command to run after ALL tasks spawned by this step (and their
    /// descendants) have completed.
    ///
    /// Receives the original task value on stdin. Outputs next tasks on stdout.
    /// Useful for cleanup, aggregation, or triggering follow-up work after a
    /// fan-out completes.
    ///
    /// Runs even if some descendants failed. Failures are logged but don't
    /// prevent the workflow from continuing.
    #[serde(default, rename = "finally")]
    pub finally_hook: Option<String>,

    /// Per-step options that override global options.
    #[serde(default)]
    pub options: StepOptions,
}

/// How a step processes tasks.
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum Action {
    /// Send to the agent pool for processing.
    Pool {
        /// Markdown instructions shown to agents.
        #[serde(default)]
        instructions: Instructions,
    },
    /// Run a local command.
    Command {
        /// Shell script to execute. Receives the task JSON on stdin,
        /// must output response JSON (array of next tasks) on stdout.
        script: String,
    },
}

impl Default for Action {
    fn default() -> Self {
        Self::Pool {
            instructions: Instructions::default(),
        }
    }
}

impl Action {
    /// Get the instructions if this is a pool action.
    #[must_use]
    pub const fn instructions(&self) -> Option<&Instructions> {
        match self {
            Self::Pool { instructions } => Some(instructions),
            Self::Command { .. } => None,
        }
    }
}

/// Per-step options that override global defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepOptions {
    /// Timeout in seconds for this step (overrides global).
    #[serde(default)]
    pub timeout: Option<u64>,

    /// Maximum retries for this step (overrides global).
    #[serde(default)]
    pub max_retries: Option<u32>,

    /// Whether to retry on timeout for this step (overrides global).
    #[serde(default)]
    pub retry_on_timeout: Option<bool>,

    /// Whether to retry on invalid response for this step (overrides global).
    #[serde(default)]
    pub retry_on_invalid_response: Option<bool>,
}

/// Resolved options for a step (global defaults merged with per-step overrides).
#[derive(Debug, Clone, Copy)]
pub struct EffectiveOptions {
    /// Timeout in seconds.
    pub timeout: Option<u64>,
    /// Maximum retries.
    pub max_retries: u32,
    /// Whether to retry on timeout.
    pub retry_on_timeout: bool,
    /// Whether to retry on invalid response.
    pub retry_on_invalid_response: bool,
}

impl EffectiveOptions {
    /// Merge global options with step-specific overrides.
    #[must_use]
    pub fn resolve(global: &Options, step: &StepOptions) -> Self {
        Self {
            timeout: step.timeout.or(global.timeout),
            max_retries: step.max_retries.unwrap_or(global.max_retries),
            retry_on_timeout: step.retry_on_timeout.unwrap_or(global.retry_on_timeout),
            retry_on_invalid_response: step
                .retry_on_invalid_response
                .unwrap_or(global.retry_on_invalid_response),
        }
    }
}

/// Reference to a JSON Schema (inline or external file).
///
/// In config files:
/// - String → link to schema file
/// - Object → inline JSON Schema
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SchemaRef {
    /// Path to a JSON Schema file.
    Link(String),
    /// Inline JSON Schema.
    Inline(serde_json::Value),
}

/// Markdown instructions (inline or external file).
///
/// In config files:
/// - String → inline markdown
/// - `{"link": "path"}` → link to markdown file
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Instructions {
    /// Inline markdown text.
    Inline(String),
    /// Link to a markdown file.
    Link {
        /// Path to the markdown file.
        link: String,
    },
}

impl Default for Instructions {
    fn default() -> Self {
        Self::Inline(String::new())
    }
}

impl Instructions {
    /// Get the inline string if this is inline instructions.
    #[must_use]
    pub fn as_inline(&self) -> Option<&str> {
        match self {
            Self::Inline(s) => Some(s),
            Self::Link { .. } => None,
        }
    }

    /// Get the link path if this is a link.
    #[must_use]
    pub fn as_link(&self) -> Option<&str> {
        match self {
            Self::Inline(_) => None,
            Self::Link { link } => Some(link),
        }
    }
}

impl Config {
    /// Build a map of step name to step for efficient lookup.
    #[must_use]
    pub fn step_map(&self) -> HashMap<&str, &Step> {
        self.steps.iter().map(|s| (s.name.as_str(), s)).collect()
    }

    /// Check if any step uses the Pool action.
    #[must_use]
    pub fn has_pool_actions(&self) -> bool {
        self.steps
            .iter()
            .any(|s| matches!(s.action, Action::Pool { .. }))
    }

    /// Validate the config for internal consistency.
    ///
    /// Checks:
    /// - Step names are unique
    /// - All `next` references point to existing steps
    ///
    /// # Errors
    ///
    /// Returns an error describing any validation failures.
    pub fn validate(&self) -> Result<(), ConfigError> {
        // Find duplicate names
        let mut seen: HashMap<&str, usize> = HashMap::new();
        for name in self.steps.iter().map(|s| &s.name) {
            *seen.entry(name.as_str()).or_insert(0) += 1;
        }
        let duplicates: Vec<StepName> = seen
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(name, _)| StepName::new(name))
            .collect();

        if !duplicates.is_empty() {
            return Err(ConfigError::DuplicateStepNames { names: duplicates });
        }

        // Check all next references are valid
        let step_names: std::collections::HashSet<&str> =
            self.steps.iter().map(|s| s.name.as_str()).collect();

        for step in &self.steps {
            for next in &step.next {
                if !step_names.contains(next.as_str()) {
                    return Err(ConfigError::InvalidNextStep {
                        from: step.name.clone(),
                        to: next.clone(),
                    });
                }
            }
        }

        // Check entrypoint references a valid step
        if let Some(ref entrypoint) = self.entrypoint
            && !step_names.contains(entrypoint.as_str())
        {
            return Err(ConfigError::InvalidEntrypoint {
                name: entrypoint.clone(),
            });
        }

        Ok(())
    }
}

/// Errors that can occur during config validation.
#[derive(Debug, Clone)]
pub enum ConfigError {
    /// Two or more steps have the same name.
    DuplicateStepNames {
        /// The step names that appear more than once.
        names: Vec<StepName>,
    },
    /// A step references a non-existent next step.
    InvalidNextStep {
        /// The step containing the invalid reference.
        from: StepName,
        /// The referenced step that doesn't exist.
        to: StepName,
    },
    /// The entrypoint references a non-existent step.
    InvalidEntrypoint {
        /// The entrypoint step name that doesn't exist.
        name: StepName,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateStepNames { names } => {
                let names_str: Vec<&str> = names.iter().map(StepName::as_str).collect();
                write!(f, "duplicate step names: {}", names_str.join(", "))
            }
            Self::InvalidNextStep { from, to } => {
                write!(f, "step '{from}' references non-existent step '{to}'")
            }
            Self::InvalidEntrypoint { name } => {
                write!(f, "entrypoint '{name}' references non-existent step")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

/// Generate JSON Schema for the Config type.
#[must_use]
pub fn config_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(Config)
}

#[cfg(test)]
#[expect(clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn parse_minimal_config() {
        let json = r#"{
            "steps": [
                {"name": "Start", "next": ["End"]},
                {"name": "End", "next": []}
            ]
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        assert_eq!(config.steps.len(), 2);
        assert!(config.options.timeout.is_none());
    }

    #[test]
    fn parse_full_config() {
        let json = r#"{
            "options": {
                "timeout": 120,
                "max_retries": 3
            },
            "steps": [
                {
                    "name": "Analyze",
                    "value_schema": {"type": "object"},
                    "action": {"kind": "Pool", "instructions": "Analyze the input."},
                    "next": ["Done"]
                },
                {
                    "name": "Done",
                    "next": []
                }
            ]
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        assert_eq!(config.options.timeout, Some(120));
        assert_eq!(config.options.max_retries, 3);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_catches_invalid_next() {
        let json = r#"{
            "steps": [
                {"name": "Start", "next": ["NonExistent"]}
            ]
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        assert!(config.validate().is_err());
    }

    #[test]
    fn empty_steps_is_valid() {
        let json = r#"{"steps": []}"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        assert!(config.validate().is_ok());
        assert_eq!(config.steps.len(), 0);
    }

    #[test]
    fn validate_catches_duplicate_step_names() {
        let json = r#"{
            "steps": [
                {"name": "Start", "next": []},
                {"name": "Start", "next": []}
            ]
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ConfigError::DuplicateStepNames { names }) if names == vec!["Start"]
        ));
    }

    #[test]
    fn retry_options_default_to_true() {
        let json = r#"{"steps": []}"#;
        let config: Config = serde_json::from_str(json).expect("parse failed");

        assert!(config.options.retry_on_timeout);
        assert!(config.options.retry_on_invalid_response);
    }

    #[test]
    fn retry_options_can_be_disabled() {
        let json = r#"{
            "options": {
                "retry_on_timeout": false,
                "retry_on_invalid_response": false
            },
            "steps": []
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        assert!(!config.options.retry_on_timeout);
        assert!(!config.options.retry_on_invalid_response);
    }

    #[test]
    fn per_step_options_override_global() {
        let json = r#"{
            "options": {
                "timeout": 60,
                "max_retries": 3,
                "retry_on_timeout": true
            },
            "steps": [{
                "name": "ExpensiveStep",
                "next": [],
                "options": {
                    "timeout": 300,
                    "max_retries": 1,
                    "retry_on_timeout": false
                }
            }]
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        let step = &config.steps[0];
        let effective = EffectiveOptions::resolve(&config.options, &step.options);

        assert_eq!(effective.timeout, Some(300));
        assert_eq!(effective.max_retries, 1);
        assert!(!effective.retry_on_timeout);
        // retry_on_invalid_response not overridden, uses global default
        assert!(effective.retry_on_invalid_response);
    }

    #[test]
    fn effective_options_uses_global_when_step_not_set() {
        let json = r#"{
            "options": {
                "timeout": 60,
                "max_retries": 5
            },
            "steps": [{
                "name": "BasicStep",
                "next": []
            }]
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        let step = &config.steps[0];
        let effective = EffectiveOptions::resolve(&config.options, &step.options);

        assert_eq!(effective.timeout, Some(60));
        assert_eq!(effective.max_retries, 5);
        assert!(effective.retry_on_timeout);
        assert!(effective.retry_on_invalid_response);
    }

    #[test]
    fn action_pool_inline_instructions() {
        let json = r#"{
            "steps": [{
                "name": "Test",
                "action": {"kind": "Pool", "instructions": "Inline markdown here."},
                "next": []
            }]
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(
            &config.steps[0].action,
            Action::Pool { instructions: Instructions::Inline(s) } if s == "Inline markdown here."
        ));
    }

    #[test]
    fn action_pool_link_instructions() {
        let json = r#"{
            "steps": [{
                "name": "Test",
                "action": {"kind": "Pool", "instructions": {"link": "path/to/instructions.md"}},
                "next": []
            }]
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(
            &config.steps[0].action,
            Action::Pool { instructions: Instructions::Link { link } } if link == "path/to/instructions.md"
        ));
    }

    #[test]
    fn action_command() {
        let json = r#"{
            "steps": [{
                "name": "Test",
                "action": {"kind": "Command", "script": "jq '.value'"},
                "next": []
            }]
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(
            &config.steps[0].action,
            Action::Command { script } if script == "jq '.value'"
        ));
    }

    #[test]
    fn action_defaults_to_pool() {
        let json = r#"{
            "steps": [{
                "name": "Test",
                "next": []
            }]
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(&config.steps[0].action, Action::Pool { .. }));
    }

    #[test]
    fn schema_inline_object() {
        let json = r#"{
            "steps": [{
                "name": "Test",
                "value_schema": {"type": "object"},
                "next": []
            }]
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(
            &config.steps[0].value_schema,
            Some(SchemaRef::Inline(_))
        ));
    }

    #[test]
    fn schema_link_string() {
        let json = r#"{
            "steps": [{
                "name": "Test",
                "value_schema": "schemas/test.json",
                "next": []
            }]
        }"#;

        let config: Config = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(
            &config.steps[0].value_schema,
            Some(SchemaRef::Link(path)) if path == "schemas/test.json"
        ));
    }
}
