//! Configuration types for GSD.
//!
//! Defines the task queue with steps, schemas, and transitions.
//! These types are serialization-format agnostic (use serde).

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level GSD configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// JSON Schema reference for editor validation (ignored at runtime).
    #[serde(rename = "$schema", default, skip_serializing)]
    pub schema_ref: Option<String>,

    /// Runtime options.
    #[serde(default)]
    pub options: Options,

    /// Step definitions forming the task queue.
    pub steps: Vec<Step>,
}

/// Runtime options for task execution.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Step name (e.g., `Analyze`, `Implement`).
    pub name: String,

    /// JSON Schema for validating the step's value payload.
    /// None means any JSON value is accepted.
    #[serde(default)]
    pub value_schema: Option<SchemaRef>,

    /// Markdown instructions shown to agents.
    #[serde(default)]
    pub instructions: String,

    /// Valid next step names (empty = terminal step).
    #[serde(default)]
    pub next: Vec<String>,

    /// Per-step options that override global options.
    #[serde(default)]
    pub options: StepOptions,
}

/// Per-step options that override global defaults.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value")]
pub enum SchemaRef {
    /// Inline JSON Schema.
    Inline(serde_json::Value),
    /// Path to a JSON Schema file.
    Link(String),
}

impl Config {
    /// Build a map of step name to step for efficient lookup.
    #[must_use]
    pub fn step_map(&self) -> HashMap<&str, &Step> {
        self.steps.iter().map(|s| (s.name.as_str(), s)).collect()
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
        let mut seen = HashMap::new();
        for name in self.steps.iter().map(|s| &s.name) {
            *seen.entry(name.as_str()).or_insert(0usize) += 1;
        }
        let duplicates: Vec<String> = seen
            .into_iter()
            .filter(|(_, count)| *count > 1)
            .map(|(name, _)| name.to_string())
            .collect();

        if !duplicates.is_empty() {
            return Err(ConfigError::DuplicateStepNames { names: duplicates });
        }

        // Check all next references are valid
        let step_names: std::collections::HashSet<_> =
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

        Ok(())
    }
}

/// Errors that can occur during config validation.
#[derive(Debug, Clone)]
pub enum ConfigError {
    /// Two or more steps have the same name.
    DuplicateStepNames {
        /// The step names that appear more than once.
        names: Vec<String>,
    },
    /// A step references a non-existent next step.
    InvalidNextStep {
        /// The step containing the invalid reference.
        from: String,
        /// The referenced step that doesn't exist.
        to: String,
    },
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateStepNames { names } => {
                write!(f, "duplicate step names: {}", names.join(", "))
            }
            Self::InvalidNextStep { from, to } => {
                write!(f, "step '{from}' references non-existent step '{to}'")
            }
        }
    }
}

impl std::error::Error for ConfigError {}

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
                    "value_schema": {"kind": "Inline", "value": {"type": "object"}},
                    "instructions": "Analyze the input.",
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
}
