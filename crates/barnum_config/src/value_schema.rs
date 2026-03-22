//! JSON Schema handling for Barnum.
//!
//! Loads schemas from config (inline or file) and validates task payloads.

use crate::resolved::{Config, Step};
use crate::types::{StepInputValue, StepName};
use jsonschema::Validator;
use std::collections::HashMap;
use std::io;

/// Compiled schemas for all steps in a config.
pub struct CompiledSchemas {
    /// Map of step name to compiled validator.
    validators: HashMap<StepName, Option<Validator>>,
}

impl CompiledSchemas {
    /// Compile all schemas from a resolved config.
    ///
    /// # Errors
    ///
    /// Returns an error if a schema is invalid.
    pub fn compile(config: &Config) -> io::Result<Self> {
        let mut validators = HashMap::new();

        for step in &config.steps {
            let validator = step.value_schema.as_ref().map(compile_schema).transpose()?;
            validators.insert(step.name.clone(), validator);
        }

        Ok(Self { validators })
    }

    /// Validate a value against a step's schema.
    ///
    /// Returns Ok if the step has no schema or the value is valid.
    ///
    /// # Errors
    ///
    /// Returns an error if the step doesn't exist or the value fails validation.
    pub fn validate(
        &self,
        step_name: &StepName,
        value: &StepInputValue,
    ) -> Result<(), ValidationError> {
        let Some(maybe_validator) = self.validators.get(step_name.as_str()) else {
            return Err(ValidationError::UnknownStep(step_name.clone()));
        };

        let Some(validator) = maybe_validator else {
            // No schema means any value is valid
            return Ok(());
        };

        if validator.is_valid(&value.0) {
            Ok(())
        } else {
            // Collect validation errors
            let errors: Vec<String> = validator
                .iter_errors(&value.0)
                .map(|e| e.to_string())
                .collect();
            Err(ValidationError::SchemaViolation {
                step: step_name.clone(),
                errors,
            })
        }
    }
}

fn compile_schema(schema: &serde_json::Value) -> io::Result<Validator> {
    Validator::new(schema).map_err(|e| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("[E050] invalid schema: {e}"),
        )
    })
}

/// Errors that can occur during validation.
#[derive(Debug, Clone)]
pub enum ValidationError {
    /// Referenced step doesn't exist.
    UnknownStep(StepName),
    /// Value doesn't match the schema.
    SchemaViolation {
        /// The step whose schema was violated.
        step: StepName,
        /// List of validation errors.
        errors: Vec<String>,
    },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownStep(name) => write!(f, "unknown step: {name}"),
            Self::SchemaViolation { step, errors } => {
                write!(
                    f,
                    "schema violation for step '{step}': {}",
                    errors.join(", ")
                )
            }
        }
    }
}

impl std::error::Error for ValidationError {}

/// A task with its kind (step name) and value.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Task {
    /// The step name (serialized as "kind" for compatibility with agent responses).
    #[serde(rename = "kind")]
    pub step: StepName,
    /// The task payload.
    pub value: StepInputValue,
    /// Number of times this task has been retried (internal tracking, not serialized).
    #[serde(skip)]
    pub(crate) retries: u32,
}

impl Task {
    /// Create a new task with the given step name and value.
    #[must_use]
    pub fn new(step: impl Into<StepName>, value: StepInputValue) -> Self {
        Self {
            step: step.into(),
            value,
            retries: 0,
        }
    }
}

/// Validate an agent's response against the config.
///
/// Checks that:
/// - Response is a JSON array
/// - Each task's kind is a valid next step from the current step
/// - Each task's value matches the target step's schema
///
/// # Errors
///
/// Returns an error if the response format is invalid, contains invalid
/// transitions, or values fail schema validation.
pub fn validate_response(
    response: &serde_json::Value,
    current_step: &Step,
    schemas: &CompiledSchemas,
) -> Result<Vec<Task>, ResponseValidationError> {
    let serde_json::Value::Array(items) = response else {
        return Err(ResponseValidationError::NotAnArray);
    };

    let mut tasks = Vec::with_capacity(items.len());

    for (i, item) in items.iter().enumerate() {
        let task: Task = serde_json::from_value(item.clone()).map_err(|e| {
            ResponseValidationError::InvalidTaskFormat {
                index: i,
                error: e.to_string(),
            }
        })?;

        // Check if this is a valid transition
        if !current_step.next.contains(&task.step) {
            return Err(ResponseValidationError::InvalidTransition {
                from: current_step.name.clone(),
                to: task.step,
                valid: current_step.next.clone(),
            });
        }

        // Validate the value against the target step's schema
        schemas
            .validate(&task.step, &task.value)
            .map_err(|e| ResponseValidationError::SchemaError { index: i, error: e })?;

        tasks.push(task);
    }

    Ok(tasks)
}

/// Errors that can occur when validating an agent response.
#[derive(Debug)]
pub enum ResponseValidationError {
    /// Response is not a JSON array.
    NotAnArray,
    /// A task in the array has invalid format.
    InvalidTaskFormat {
        /// Index of the invalid task.
        index: usize,
        /// Parse error message.
        error: String,
    },
    /// Task step is not a valid transition from current step.
    InvalidTransition {
        /// Current step name.
        from: StepName,
        /// Attempted next step.
        to: StepName,
        /// List of valid next steps.
        valid: Vec<StepName>,
    },
    /// Task value doesn't match target step's schema.
    SchemaError {
        /// Index of the invalid task.
        index: usize,
        /// Validation error.
        error: ValidationError,
    },
}

impl std::fmt::Display for ResponseValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotAnArray => write!(f, "response must be a JSON array"),
            Self::InvalidTaskFormat { index, error } => {
                write!(f, "task at index {index} has invalid format: {error}")
            }
            Self::InvalidTransition { from, to, valid } => {
                let valid_str: Vec<&str> = valid.iter().map(StepName::as_str).collect();
                write!(
                    f,
                    "invalid transition from '{from}' to '{to}' (valid: {})",
                    valid_str.join(", ")
                )
            }
            Self::SchemaError { index, error } => {
                write!(f, "task at index {index}: {error}")
            }
        }
    }
}

impl std::error::Error for ResponseValidationError {}

#[cfg(test)]
#[expect(clippy::expect_used)]
mod tests {
    use super::*;
    use crate::config::ConfigFile;
    use std::path::Path;

    const POOL: &str = r#"{"kind": "Pool", "instructions": {"kind": "Inline", "value": ""}}"#;

    fn test_config() -> Config {
        let json = format!(
            r#"{{
            "steps": [
                {{
                    "name": "Start",
                    "action": {POOL},
                    "value_schema": {{"type": "object", "properties": {{"x": {{"type": "number"}}}}}},
                    "next": ["End"]
                }},
                {{
                    "name": "End",
                    "action": {POOL},
                    "next": []
                }}
            ]
        }}"#
        );
        let config_file: ConfigFile = serde_json::from_str(&json).expect("parse config");
        config_file.resolve(Path::new(".")).expect("resolve config")
    }

    #[test]
    fn validates_correct_value() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

        let value = StepInputValue(serde_json::json!({"x": 42}));
        assert!(schemas.validate(&StepName::new("Start"), &value).is_ok());
    }

    #[test]
    fn rejects_invalid_value() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

        let value = StepInputValue(serde_json::json!({"x": "not a number"}));
        assert!(schemas.validate(&StepName::new("Start"), &value).is_err());
    }

    #[test]
    fn accepts_any_value_without_schema() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config).expect("compile schemas");

        // End step has no schema
        let value = StepInputValue(serde_json::json!({"anything": "goes"}));
        assert!(schemas.validate(&StepName::new("End"), &value).is_ok());
    }

    #[test]
    fn validate_response_accepts_valid_array() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
        let step = &config.steps[0]; // Start step, next = ["End"]

        let response = serde_json::json!([
            {"kind": "End", "value": {}}
        ]);

        let tasks = validate_response(&response, step, &schemas).expect("should be valid");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].step, "End");
    }

    #[test]
    fn validate_response_rejects_non_array() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
        let step = &config.steps[0];

        let response = serde_json::json!({"kind": "End", "value": {}});

        let result = validate_response(&response, step, &schemas);
        assert!(matches!(result, Err(ResponseValidationError::NotAnArray)));
    }

    #[test]
    fn validate_response_rejects_invalid_transition() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
        let step = &config.steps[0]; // Start step, next = ["End"]

        // Try to transition to Start (not allowed from Start)
        let response = serde_json::json!([
            {"kind": "Start", "value": {"x": 1}}
        ]);

        let result = validate_response(&response, step, &schemas);
        assert!(matches!(
            result,
            Err(ResponseValidationError::InvalidTransition { .. })
        ));
    }

    #[test]
    fn validate_response_accepts_empty_array() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config).expect("compile schemas");
        let step = &config.steps[1]; // End step (terminal)

        let response = serde_json::json!([]);

        let tasks = validate_response(&response, step, &schemas).expect("should be valid");
        assert!(tasks.is_empty());
    }
}
