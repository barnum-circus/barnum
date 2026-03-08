//! Shared domain types for GSD crates.
//!
//! These types are used across multiple GSD crates (`gsd_config`, `gsd_state`, etc.)
//! to provide semantic clarity and type safety.

use serde::{Deserialize, Serialize};
use string_id::define_string_id;

/// Unique identifier for a task instance within a GSD run.
///
/// Used both at runtime (in the runner) and for serialization (in state logs).
/// Named `LogTaskId` to avoid confusion with `agent_pool::TaskId` which is unrelated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct LogTaskId(pub u32);

define_string_id! {
    /// Step name identifier (the `name` field in configs, `kind` in tasks).
    pub struct StepName;
}

define_string_id! {
    /// Shell command for hooks (pre, post, finally).
    pub struct HookScript;
}

/// A step's input value - the JSON payload passed to/from steps.
///
/// All step values in the system use this type, whether they've been
/// through a pre-hook transformation or not. The transformation is optional,
/// so there's no meaningful type distinction between "before" and "after".
#[derive(Debug, Clone, Serialize, Deserialize)]
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
