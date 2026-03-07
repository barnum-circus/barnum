//! Domain types for GSD config.
//!
//! These newtypes add semantic clarity to commonly confused string types.

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
