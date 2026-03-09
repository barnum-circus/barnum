//! Configuration types for GSD.
//!
//! Defines the task queue with steps, schemas, and transitions.
//! These types are serialization-format agnostic (use serde).

use crate::types::{HookScript, StepName};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level GSD configuration file format.
///
/// This is the raw parsed format. Call `resolve()` to get the runtime `Config`.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
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
    pub steps: Vec<StepFile>,
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

/// A step in the config file (may have unresolved references).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepFile {
    /// Step name (e.g., `Analyze`, `Implement`).
    pub name: StepName,

    /// JSON Schema for validating the step's value payload.
    /// None means any JSON value is accepted.
    #[serde(default)]
    pub value_schema: Option<SchemaRef>,

    /// Pre-execution hook script.
    #[serde(default)]
    pub pre: Option<HookScript>,

    /// How to execute the step.
    #[serde(default)]
    pub action: ActionFile,

    /// Post-execution hook script.
    #[serde(default)]
    pub post: Option<HookScript>,

    /// Valid next steps.
    #[serde(default)]
    pub next: Vec<StepName>,

    /// Finally hook (runs after all children complete).
    #[serde(default, rename = "finally")]
    pub finally_hook: Option<HookScript>,

    /// Per-step options that override global options.
    #[serde(default)]
    pub options: StepOptions,
}

/// How a step processes tasks (config file format).
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum ActionFile {
    /// Send to the agent pool for processing.
    Pool {
        /// Markdown instructions shown to agents.
        #[serde(default)]
        instructions: crate::maybe_linked::MaybeLinked<Instructions>,
    },
    /// Run a local command.
    Command {
        /// Shell script to execute.
        ///
        /// **Input (stdin):** JSON object with the structure `{"kind": "<step name>", "value": <parameters>}`.
        /// Use `jq '.value'` to extract parameters or `jq -r '.value.fieldName'` for specific fields.
        ///
        /// **Output (stdout):** JSON array of next tasks, e.g. `[{"kind": "NextStep", "value": {...}}]`.
        /// Return `[]` to spawn no follow-up tasks.
        script: String,
    },
}

impl Default for ActionFile {
    fn default() -> Self {
        Self::Pool {
            instructions: crate::maybe_linked::MaybeLinked::default(),
        }
    }
}

impl ActionFile {
    /// Get the instructions if this is a pool action.
    #[must_use]
    pub const fn instructions(&self) -> Option<&crate::maybe_linked::MaybeLinked<Instructions>> {
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
/// - `{"link": "path"}` → link to schema file
/// - Object → inline JSON Schema
#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum SchemaRef {
    /// Link to a JSON Schema file.
    Link {
        /// Path to the schema file.
        link: String,
    },
    /// Inline JSON Schema.
    Inline(serde_json::Value),
}

/// Markdown instructions content.
#[derive(Debug, Serialize, Deserialize, JsonSchema, Default, PartialEq, Eq)]
#[serde(transparent)]
pub struct Instructions(pub String);

impl ConfigFile {
    /// Build a map of step name to step for efficient lookup.
    #[must_use]
    pub fn step_map(&self) -> HashMap<&StepName, &StepFile> {
        self.steps.iter().map(|s| (&s.name, s)).collect()
    }

    /// Check if any step uses the Pool action.
    #[must_use]
    pub fn has_pool_actions(&self) -> bool {
        self.steps
            .iter()
            .any(|s| matches!(s.action, ActionFile::Pool { .. }))
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

    /// Resolve all file references and compute effective options.
    ///
    /// Returns a fully resolved `Config` ready for runtime use.
    ///
    /// # Errors
    ///
    /// Returns an error if any linked file cannot be read.
    pub fn resolve(self, base_path: &std::path::Path) -> std::io::Result<crate::resolved::Config> {
        let global_options = &self.options;
        let steps = self
            .steps
            .into_iter()
            .map(|step| step.resolve(base_path, global_options))
            .collect::<std::io::Result<Vec<_>>>()?;

        Ok(crate::resolved::Config {
            max_concurrency: self.options.max_concurrency,
            steps,
        })
    }
}

impl StepFile {
    /// Resolve this step's file references and compute effective options.
    fn resolve(
        self,
        base_path: &std::path::Path,
        global_options: &Options,
    ) -> std::io::Result<crate::resolved::Step> {
        let action = self.action.resolve(base_path)?;
        let value_schema = self
            .value_schema
            .map(|s| resolve_schema(s, base_path))
            .transpose()?;
        let options = EffectiveOptions::resolve(global_options, &self.options);

        Ok(crate::resolved::Step {
            name: self.name,
            value_schema,
            pre: self.pre,
            action,
            post: self.post,
            next: self.next,
            finally_hook: self.finally_hook,
            options: crate::resolved::Options {
                timeout: options.timeout,
                max_retries: options.max_retries,
                retry_on_timeout: options.retry_on_timeout,
                retry_on_invalid_response: options.retry_on_invalid_response,
            },
        })
    }
}

impl ActionFile {
    /// Resolve this action's file references.
    fn resolve(self, base_path: &std::path::Path) -> std::io::Result<crate::resolved::Action> {
        match self {
            Self::Pool { instructions } => {
                let resolved: Instructions = instructions.resolve(base_path, |path| {
                    let content = std::fs::read_to_string(path)?;
                    Ok(Instructions(content))
                })?;
                Ok(crate::resolved::Action::Pool {
                    instructions: resolved.0,
                })
            }
            Self::Command { script } => Ok(crate::resolved::Action::Command { script }),
        }
    }
}

/// Resolve a schema reference to its JSON value.
fn resolve_schema(
    schema: SchemaRef,
    base_path: &std::path::Path,
) -> std::io::Result<serde_json::Value> {
    match schema {
        SchemaRef::Inline(value) => Ok(value),
        SchemaRef::Link { link } => {
            let path = base_path.join(&link);
            let content = std::fs::read_to_string(&path).map_err(|e| {
                std::io::Error::new(
                    e.kind(),
                    format!("failed to read schema '{}': {e}", path.display()),
                )
            })?;
            serde_json::from_str(&content).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("invalid JSON in schema '{}': {e}", path.display()),
                )
            })
        }
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

/// Generate JSON Schema for the `ConfigFile` type.
#[must_use]
pub fn config_schema() -> schemars::schema::RootSchema {
    schemars::schema_for!(ConfigFile)
}

#[cfg(test)]
#[expect(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::maybe_linked::MaybeLinked;

    #[test]
    fn parse_minimal_config() {
        let json = r#"{
            "steps": [
                {"name": "Start", "next": ["End"]},
                {"name": "End", "next": []}
            ]
        }"#;

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
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
                    "action": {"kind": "Pool", "instructions": {"inline": "Analyze the input."}},
                    "next": ["Done"]
                },
                {
                    "name": "Done",
                    "next": []
                }
            ]
        }"#;

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
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

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
        assert!(config.validate().is_err());
    }

    #[test]
    fn empty_steps_is_valid() {
        let json = r#"{"steps": []}"#;

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
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

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
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
        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");

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

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
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

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
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

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
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
                "action": {"kind": "Pool", "instructions": {"inline": "Inline markdown here."}},
                "next": []
            }]
        }"#;

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(
            &config.steps[0].action,
            ActionFile::Pool { instructions: MaybeLinked::Inline { inline: Instructions(s) } } if s == "Inline markdown here."
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

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(
            &config.steps[0].action,
            ActionFile::Pool { instructions: MaybeLinked::Link { link } } if link == "path/to/instructions.md"
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

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(
            &config.steps[0].action,
            ActionFile::Command { script } if script == "jq '.value'"
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

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(&config.steps[0].action, ActionFile::Pool { .. }));
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

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(
            &config.steps[0].value_schema,
            Some(SchemaRef::Inline(_))
        ));
    }

    #[test]
    fn schema_link_object() {
        let json = r#"{
            "steps": [{
                "name": "Test",
                "value_schema": {"link": "schemas/test.json"},
                "next": []
            }]
        }"#;

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(
            &config.steps[0].value_schema,
            Some(SchemaRef::Link { link }) if link == "schemas/test.json"
        ));
    }
}
