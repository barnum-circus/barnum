//! Shared domain types for Barnum crates.
//!
//! These types are used across multiple Barnum crates (`barnum_config`, `barnum_state`, etc.)
//! to provide semantic clarity and type safety.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use string_id::define_string_id;

/// Unique identifier for a task instance within a Barnum run.
///
/// Used both at runtime (in the runner) and for serialization (in state logs).
/// Named `LogTaskId` to avoid confusion with `troupe::TaskId` which is unrelated.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, JsonSchema,
)]
#[serde(transparent)]
pub struct LogTaskId(pub u32);

define_string_id! {
    /// A step's unique name (e.g., `"Analyze"`, `"Implement"`). This string
    /// appears as the `name` field in config files and as the `kind` field
    /// when creating follow-up tasks: `{"kind": "StepName", "value": {...}}`.
    pub struct StepName;
}

/// A step's input value - the JSON payload passed to/from steps.
///
/// All step values in the system use this type, whether they've been
/// through a pre-hook transformation or not. The transformation is optional,
/// so there's no meaningful type distinction between "before" and "after".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(transparent)]
pub struct StepInputValue(pub serde_json::Value);

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn step_name_equality() {
        let name = StepName::new("Analyze");
        assert_eq!(name, "Analyze");
        assert_eq!(name, "Analyze".to_string());
        assert_eq!(name.as_str(), "Analyze");
    }

    #[test]
    fn step_name_serializes_transparently() {
        let name = StepName::new("Test");
        let json = serde_json::to_string(&name).unwrap();
        assert_eq!(json, "\"Test\"");

        let parsed: StepName = serde_json::from_str("\"Test\"").unwrap();
        assert_eq!(parsed, name);
    }

    #[test]
    fn step_name_in_hashmap() {
        use std::collections::HashMap;

        let mut map: HashMap<StepName, i32> = HashMap::new();
        map.insert(StepName::new("A"), 1);

        // Can lookup with &str via Borrow
        assert_eq!(map.get("A"), Some(&1));
    }
}
