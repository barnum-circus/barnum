//! JSON Schema handling for GSD.
//!
//! Loads schemas from config (inline or file) and validates task payloads.

use crate::config::{Config, SchemaRef, Step};
use jsonschema::Validator;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::{fs, io};

/// Compiled schemas for all steps in a config.
pub struct CompiledSchemas {
    /// Map of step name to compiled validator.
    validators: HashMap<String, Option<Validator>>,
}

impl CompiledSchemas {
    /// Compile all schemas from a config.
    ///
    /// # Errors
    ///
    /// Returns an error if a schema file can't be read or a schema is invalid.
    pub fn compile(config: &Config, base_path: &Path) -> io::Result<Self> {
        let mut validators = HashMap::new();

        for step in &config.steps {
            let validator = match &step.schema {
                None => None,
                Some(SchemaRef::Inline(schema)) => Some(compile_schema(schema)?),
                Some(SchemaRef::Link(path)) => {
                    let full_path = base_path.join(path);
                    let content = fs::read_to_string(&full_path)?;
                    let schema: Value = serde_json::from_str(&content)
                        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
                    Some(compile_schema(&schema)?)
                }
            };
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
    pub fn validate(&self, step_name: &str, value: &Value) -> Result<(), ValidationError> {
        let Some(maybe_validator) = self.validators.get(step_name) else {
            return Err(ValidationError::UnknownStep(step_name.to_string()));
        };

        let Some(validator) = maybe_validator else {
            // No schema means any value is valid
            return Ok(());
        };

        if validator.is_valid(value) {
            Ok(())
        } else {
            // Collect validation errors
            let errors: Vec<String> = validator
                .iter_errors(value)
                .map(|e| e.to_string())
                .collect();
            Err(ValidationError::SchemaViolation {
                step: step_name.to_string(),
                errors,
            })
        }
    }
}

fn compile_schema(schema: &Value) -> io::Result<Validator> {
    Validator::new(schema)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("invalid schema: {e}")))
}

/// Errors that can occur during validation.
#[derive(Debug, Clone)]
pub enum ValidationError {
    /// Referenced step doesn't exist.
    UnknownStep(String),
    /// Value doesn't match the schema.
    SchemaViolation {
        /// The step whose schema was violated.
        step: String,
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
    /// The step/kind name.
    pub kind: String,
    /// The task payload.
    pub value: Value,
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
    response: &Value,
    current_step: &Step,
    schemas: &CompiledSchemas,
) -> Result<Vec<Task>, ResponseValidationError> {
    let Value::Array(items) = response else {
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
        if !current_step.next.contains(&task.kind) {
            return Err(ResponseValidationError::InvalidTransition {
                from: current_step.name.clone(),
                to: task.kind,
                valid: current_step.next.clone(),
            });
        }

        // Validate the value against the target step's schema
        schemas
            .validate(&task.kind, &task.value)
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
    /// Task kind is not a valid transition from current step.
    InvalidTransition {
        /// Current step name.
        from: String,
        /// Attempted next step.
        to: String,
        /// List of valid next steps.
        valid: Vec<String>,
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
                write!(
                    f,
                    "invalid transition from '{from}' to '{to}' (valid: {})",
                    valid.join(", ")
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
    use crate::config::Config;

    fn test_config() -> Config {
        serde_json::from_str(
            r#"{
            "steps": [
                {
                    "name": "Start",
                    "schema": {"kind": "Inline", "value": {"type": "object", "properties": {"x": {"type": "number"}}}},
                    "next": ["End"]
                },
                {
                    "name": "End",
                    "next": []
                }
            ]
        }"#,
        )
        .expect("parse config")
    }

    #[test]
    fn validates_correct_value() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");

        let value = serde_json::json!({"x": 42});
        assert!(schemas.validate("Start", &value).is_ok());
    }

    #[test]
    fn rejects_invalid_value() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");

        let value = serde_json::json!({"x": "not a number"});
        assert!(schemas.validate("Start", &value).is_err());
    }

    #[test]
    fn accepts_any_value_without_schema() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");

        // End step has no schema
        let value = serde_json::json!({"anything": "goes"});
        assert!(schemas.validate("End", &value).is_ok());
    }

    #[test]
    fn validate_response_accepts_valid_array() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
        let step = &config.steps[0]; // Start step, next = ["End"]

        let response = serde_json::json!([
            {"kind": "End", "value": {}}
        ]);

        let tasks = validate_response(&response, step, &schemas).expect("should be valid");
        assert_eq!(tasks.len(), 1);
        assert_eq!(tasks[0].kind, "End");
    }

    #[test]
    fn validate_response_rejects_non_array() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
        let step = &config.steps[0];

        let response = serde_json::json!({"kind": "End", "value": {}});

        let result = validate_response(&response, step, &schemas);
        assert!(matches!(result, Err(ResponseValidationError::NotAnArray)));
    }

    #[test]
    fn validate_response_rejects_invalid_transition() {
        let config = test_config();
        let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
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
        let schemas = CompiledSchemas::compile(&config, Path::new(".")).expect("compile schemas");
        let step = &config.steps[1]; // End step (terminal)

        let response = serde_json::json!([]);

        let tasks = validate_response(&response, step, &schemas).expect("should be valid");
        assert!(tasks.is_empty());
    }
}
