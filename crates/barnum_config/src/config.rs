//! Configuration types for Barnum.
//!
//! Defines the task queue with steps, schemas, and transitions.
//! These types are serialization-format agnostic (use serde).

use crate::types::{HookScript, StepName};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Top-level Barnum configuration file format.
///
/// Defines a workflow as a directed graph of steps. Each step processes tasks
/// and can spawn follow-up tasks on other steps.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ConfigFile {
    /// Optional JSON Schema URL for editor validation (e.g.,
    /// `"./node_modules/@barnum/barnum/barnum-config-schema.json"`). Ignored at runtime.
    #[serde(rename = "$schema", default, skip_serializing)]
    pub schema_ref: Option<String>,

    /// Global runtime options (timeout, retries, concurrency). Individual steps
    /// can override these via their own `options` field.
    #[serde(default)]
    pub options: Options,

    /// Name of the step that starts the workflow. When set, the CLI accepts
    /// `--entrypoint-value` to provide the initial task value (defaults to `{}`).
    /// When omitted, `--initial-state` must provide explicit `[{"kind": "StepName", "value": ...}]` tasks.
    #[serde(default)]
    pub entrypoint: Option<StepName>,

    /// The steps that make up this workflow. Each step defines how to process
    /// a task and which steps it can spawn follow-up tasks on.
    pub steps: Vec<StepFile>,
}

/// Global runtime options for task execution. All fields have sensible defaults.
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

/// A named step in the workflow. Steps are the nodes of the task graph.
///
/// The `finally` hook runs after the task **and all of its descendant tasks** complete.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepFile {
    /// Unique name for this step (e.g., `"Analyze"`, `"Implement"`, `"Review"`).
    /// This is the string used as `kind` when creating tasks:
    /// `{"kind": "ThisStepName", "value": {...}}`.
    pub name: StepName,

    /// How this step processes tasks.
    pub action: ActionFile,

    /// Step names this step is allowed to spawn follow-up tasks on.
    /// Each string must match the `name` of another step in this config.
    /// An empty array means this is a terminal step (no follow-ups).
    #[serde(default)]
    pub next: Vec<StepName>,

    /// Shell script that runs after this task **and all tasks it spawned
    /// (recursively)** have completed.
    ///
    /// **stdin:** JSON object: `{"kind": "<step name>", "value": <payload>}`.
    /// Same envelope format as command action scripts.
    ///
    /// **stdout:** A JSON array of follow-up tasks to spawn:
    /// `[{"kind": "StepName", "value": {...}}, ...]`. Each `kind` must be a
    /// valid step name. Return `[]` to spawn no follow-ups.
    ///
    /// Use this for cleanup, aggregation, or spawning a final summarization
    /// step after an entire subtree of work completes.
    #[serde(default, rename = "finally")]
    pub finally_hook: Option<FinallyHook>,

    /// Per-step options that override the global `options`. Only the fields
    /// you set here take effect; everything else falls through to the global defaults.
    #[serde(default)]
    pub options: StepOptions,
}

/// Run a shell command to process tasks.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct CommandActionFile {
    /// Shell script to execute.
    ///
    /// **Input (stdin):** JSON object: `{"kind": "<step name>", "value": <payload>}`.
    /// Use `jq '.value'` to extract the payload, or `jq -r '.value.fieldName'` for a specific field.
    ///
    /// **Output (stdout):** JSON array of follow-up tasks to spawn:
    /// `[{"kind": "NextStep", "value": {...}}, ...]`. Each `kind` must be a step name
    /// listed in this step's `next` array. Return `[]` to spawn no follow-ups.
    pub script: String,
}

/// How a step processes tasks.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "params")]
pub enum ActionFile {
    /// Run a shell command.
    Command(CommandActionFile),
}

/// A shell command used as a hook.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct HookCommand {
    /// Shell script to execute.
    pub script: String,
}

/// Finally hook. Runs after a task and all its descendants complete.
///
/// In JSON: `{"kind": "Command", "params": {"script": "./finally-hook.sh"}}`
///
/// **stdin:** JSON object: `{"kind": "<step name>", "value": <payload>}`.
/// **stdout:** JSON array of follow-up tasks: `[{"kind": "StepName", "value": {...}}, ...]`.
/// Return `[]` for no follow-ups.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", content = "params")]
pub enum FinallyHook {
    /// Run a shell command as the finally hook.
    Command(HookCommand),
}

/// Per-step option overrides. Only set the fields you want to override;
/// omitted fields inherit from the global `options`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct StepOptions {
    /// Timeout in seconds for tasks on this step (overrides global `timeout`).
    #[serde(default)]
    pub timeout: Option<u64>,

    /// Maximum retries for tasks on this step (overrides global `max_retries`).
    #[serde(default)]
    pub max_retries: Option<u32>,

    /// Whether to retry when an agent times out on this step (overrides global `retry_on_timeout`).
    #[serde(default)]
    pub retry_on_timeout: Option<bool>,

    /// Whether to retry when an agent returns an invalid response on this step
    /// (overrides global `retry_on_invalid_response`).
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

impl ConfigFile {
    /// Build a map of step name to step for efficient lookup.
    #[must_use]
    pub fn step_map(&self) -> HashMap<&StepName, &StepFile> {
        self.steps.iter().map(|s| (&s.name, s)).collect()
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
            return Err(ConfigError::DuplicateStepNames(DuplicateStepNames {
                names: duplicates,
            }));
        }

        // Check all next references are valid
        let step_names: std::collections::HashSet<&str> =
            self.steps.iter().map(|s| s.name.as_str()).collect();

        for step in &self.steps {
            for next in &step.next {
                if !step_names.contains(next.as_str()) {
                    return Err(ConfigError::InvalidNextStep(InvalidNextStep {
                        from: step.name.clone(),
                        to: next.clone(),
                    }));
                }
            }
        }

        // Check entrypoint references a valid step
        if let Some(ref entrypoint) = self.entrypoint
            && !step_names.contains(entrypoint.as_str())
        {
            return Err(ConfigError::InvalidEntrypoint(InvalidEntrypoint {
                name: entrypoint.clone(),
            }));
        }

        Ok(())
    }

    /// Resolve all file references and compute effective options.
    ///
    /// Returns a fully resolved `Config` ready for runtime use.
    #[must_use]
    pub fn resolve(self, base_path: &std::path::Path) -> crate::resolved::Config {
        let global_options = &self.options;
        let steps = self
            .steps
            .into_iter()
            .map(|step| step.resolve(base_path, global_options))
            .collect();

        crate::resolved::Config {
            max_concurrency: self.options.max_concurrency,
            steps,
        }
    }
}

impl StepFile {
    /// Resolve this step's file references and compute effective options.
    fn resolve(
        self,
        base_path: &std::path::Path,
        global_options: &Options,
    ) -> crate::resolved::Step {
        let action = self.action.resolve(base_path);
        let options = EffectiveOptions::resolve(global_options, &self.options);

        crate::resolved::Step {
            name: self.name,
            action,
            next: self.next,
            finally_hook: self.finally_hook.map(|h| {
                let FinallyHook::Command(HookCommand { script }) = h;
                HookScript::new(script)
            }),
            options: crate::resolved::Options {
                timeout: options.timeout,
                max_retries: options.max_retries,
                retry_on_timeout: options.retry_on_timeout,
                retry_on_invalid_response: options.retry_on_invalid_response,
            },
        }
    }
}

impl ActionFile {
    /// Resolve this action's file references.
    fn resolve(self, _base_path: &std::path::Path) -> crate::resolved::ActionKind {
        match self {
            Self::Command(CommandActionFile { script }) => {
                crate::resolved::ActionKind::Command(crate::resolved::CommandAction { script })
            }
        }
    }
}

/// Two or more steps have the same name.
#[derive(Debug, Clone)]
pub struct DuplicateStepNames {
    /// The step names that appear more than once.
    pub names: Vec<StepName>,
}

/// A step references a non-existent next step.
#[derive(Debug, Clone)]
pub struct InvalidNextStep {
    /// The step containing the invalid reference.
    pub from: StepName,
    /// The referenced step that doesn't exist.
    pub to: StepName,
}

/// The entrypoint references a non-existent step.
#[derive(Debug, Clone)]
pub struct InvalidEntrypoint {
    /// The entrypoint step name that doesn't exist.
    pub name: StepName,
}

/// Errors that can occur during config validation.
#[derive(Debug, Clone)]
pub enum ConfigError {
    /// Two or more steps have the same name.
    DuplicateStepNames(DuplicateStepNames),
    /// A step references a non-existent next step.
    InvalidNextStep(InvalidNextStep),
    /// The entrypoint references a non-existent step.
    InvalidEntrypoint(InvalidEntrypoint),
}

impl std::fmt::Display for ConfigError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DuplicateStepNames(DuplicateStepNames { names }) => {
                let names_str: Vec<&str> = names.iter().map(StepName::as_str).collect();
                write!(f, "duplicate step names: {}", names_str.join(", "))
            }
            Self::InvalidNextStep(InvalidNextStep { from, to }) => {
                write!(f, "step '{from}' references non-existent step '{to}'")
            }
            Self::InvalidEntrypoint(InvalidEntrypoint { name }) => {
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

    const CMD: &str = r#"{"kind": "Command", "params": {"script": "echo '[]'"}}"#;

    /// Helper to build a step JSON with required action field.
    fn step(name: &str, next: &[&str]) -> String {
        let next_json: Vec<String> = next.iter().map(|n| format!("\"{n}\"")).collect();
        format!(
            r#"{{"name": "{name}", "action": {CMD}, "next": [{}]}}"#,
            next_json.join(", ")
        )
    }

    #[test]
    fn parse_minimal_config() {
        let json = format!(
            r#"{{"steps": [{}, {}]}}"#,
            step("Start", &["End"]),
            step("End", &[])
        );

        let config: ConfigFile = serde_json::from_str(&json).expect("parse failed");
        assert_eq!(config.steps.len(), 2);
        assert!(config.options.timeout.is_none());
    }

    #[test]
    fn parse_full_config() {
        let json = format!(
            r#"{{
            "options": {{
                "timeout": 120,
                "max_retries": 3
            }},
            "steps": [
                {{
                    "name": "Analyze",
                    "action": {{"kind": "Command", "params": {{"script": "echo '[]'"}}}},
                    "next": ["Done"]
                }},
                {}
            ]
        }}"#,
            step("Done", &[])
        );

        let config: ConfigFile = serde_json::from_str(&json).expect("parse failed");
        assert_eq!(config.options.timeout, Some(120));
        assert_eq!(config.options.max_retries, 3);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn validate_catches_invalid_next() {
        let json = format!(r#"{{"steps": [{}]}}"#, step("Start", &["NonExistent"]));

        let config: ConfigFile = serde_json::from_str(&json).expect("parse failed");
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
        let json = format!(
            r#"{{"steps": [{}, {}]}}"#,
            step("Start", &[]),
            step("Start", &[])
        );

        let config: ConfigFile = serde_json::from_str(&json).expect("parse failed");
        let result = config.validate();
        assert!(result.is_err());
        assert!(matches!(
            result,
            Err(ConfigError::DuplicateStepNames(DuplicateStepNames { names })) if names == vec!["Start"]
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
        let json = format!(
            r#"{{
            "options": {{
                "timeout": 60,
                "max_retries": 3,
                "retry_on_timeout": true
            }},
            "steps": [{{
                "name": "ExpensiveStep",
                "action": {CMD},
                "next": [],
                "options": {{
                    "timeout": 300,
                    "max_retries": 1,
                    "retry_on_timeout": false
                }}
            }}]
        }}"#
        );

        let config: ConfigFile = serde_json::from_str(&json).expect("parse failed");
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
        let json = format!(
            r#"{{
            "options": {{
                "timeout": 60,
                "max_retries": 5
            }},
            "steps": [{}]
        }}"#,
            step("BasicStep", &[])
        );

        let config: ConfigFile = serde_json::from_str(&json).expect("parse failed");
        let step = &config.steps[0];
        let effective = EffectiveOptions::resolve(&config.options, &step.options);

        assert_eq!(effective.timeout, Some(60));
        assert_eq!(effective.max_retries, 5);
        assert!(effective.retry_on_timeout);
        assert!(effective.retry_on_invalid_response);
    }

    #[test]
    fn action_command() {
        let json = r#"{
            "steps": [{
                "name": "Test",
                "action": {"kind": "Command", "params": {"script": "jq '.value'"}},
                "next": []
            }]
        }"#;

        let config: ConfigFile = serde_json::from_str(json).expect("parse failed");
        assert!(matches!(
            &config.steps[0].action,
            ActionFile::Command(CommandActionFile { script }) if script == "jq '.value'"
        ));
    }

    #[test]
    fn action_is_required() {
        let json = r#"{
            "steps": [{
                "name": "Test",
                "next": []
            }]
        }"#;

        let result = serde_json::from_str::<ConfigFile>(json);
        assert!(result.is_err(), "Omitting action should fail to parse");
    }
}
