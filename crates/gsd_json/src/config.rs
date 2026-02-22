//! Configuration parsing for GSD.
//!
//! The config file defines a state machine with steps, schemas, and transitions.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::{fs, io};

/// Top-level GSD configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    /// Runtime options.
    #[serde(default)]
    pub options: Options,

    /// Step definitions forming the state machine.
    pub steps: Vec<Step>,
}

/// Runtime options for task execution.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Options {
    /// Timeout in seconds for each task (None = no timeout).
    #[serde(default)]
    pub timeout: Option<u64>,

    /// Maximum retries for timed-out tasks (default: 0).
    #[serde(default)]
    pub max_retries: u32,
}

/// A step in the state machine.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Step {
    /// Step name (e.g., `Analyze`, `Implement`).
    pub name: String,

    /// JSON Schema for the step's value payload.
    /// None means any JSON value is accepted.
    #[serde(default)]
    pub schema: Option<SchemaRef>,

    /// Markdown instructions shown to agents.
    #[serde(default)]
    pub instructions: String,

    /// Valid next step names (empty = terminal step).
    #[serde(default)]
    pub next: Vec<String>,
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
    /// Load config from a JSON file.
    ///
    /// # Errors
    ///
    /// Returns an error if the file can't be read or contains invalid JSON.
    pub fn load(path: impl AsRef<Path>) -> io::Result<Self> {
        let content = fs::read_to_string(path)?;
        serde_json::from_str(&content).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
    }

    /// Build a map of step name to step for efficient lookup.
    #[must_use]
    pub fn step_map(&self) -> HashMap<&str, &Step> {
        self.steps.iter().map(|s| (s.name.as_str(), s)).collect()
    }

    /// Validate the config for internal consistency.
    ///
    /// Checks:
    /// - All `next` references point to existing steps
    /// - Step names are unique
    ///
    /// # Errors
    ///
    /// Returns an error describing any validation failures.
    pub fn validate(&self) -> Result<(), ConfigError> {
        let step_names: std::collections::HashSet<_> =
            self.steps.iter().map(|s| s.name.as_str()).collect();

        // Check for duplicate names
        if step_names.len() != self.steps.len() {
            return Err(ConfigError::DuplicateStepNames);
        }

        // Check all next references are valid
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
    DuplicateStepNames,
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
            Self::DuplicateStepNames => write!(f, "duplicate step names in config"),
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
                    "schema": {"kind": "Inline", "value": {"type": "object"}},
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
        assert!(matches!(result, Err(ConfigError::DuplicateStepNames)));
    }
}
