//! Workflow algebra AST types for Barnum.
//!
//! This crate defines the core data model: the [`Action`] enum (a workflow
//! program expressed as a tree of compositional nodes) and the [`Config`]
//! struct (the top-level container that pairs a workflow entry point with
//! named steps for mutual recursion).
//!
//! TypeScript builds these structures via builder functions and serializes
//! them to JSON. Rust deserializes and interprets them.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A single node in the workflow AST.
///
/// Discriminated on `kind` for JSON serialization (`#[serde(tag = "kind")]`).
/// See `refactors/pending/WORKFLOW_ALGEBRA.md` for the full specification of
/// each variant's semantics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Action {
    /// Leaf node. Invokes an exported function in a TypeScript module.
    Call {
        /// Module path (absolute).
        module: String,
        /// Exported function name.
        func: String,
        /// Optional per-step configuration forwarded to the handler.
        #[serde(default, rename = "stepConfig")]
        step_config: Option<Value>,
        /// Optional JSON Schema describing the handler's expected input.
        #[serde(default, rename = "valueSchema")]
        value_schema: Option<Value>,
    },

    /// Sequential composition. Each action receives the previous action's output.
    Sequence {
        /// Ordered list of actions to execute.
        actions: Vec<Action>,
    },

    /// Parallel map over an array input. Applies the action to each element.
    Traverse {
        /// The action to apply to each element.
        action: Box<Action>,
    },

    /// Parallel fanout. Passes the same input to all actions, collects results
    /// as an array.
    All {
        /// Independent actions to execute in parallel.
        actions: Vec<Action>,
    },

    /// N-ary branch on the `kind` field of a discriminated union input.
    Match {
        /// Map from variant `kind` values to actions.
        cases: HashMap<String, Action>,
    },

    /// Monadic fixed-point iteration. Repeats the body until it signals
    /// `Break`.
    Loop {
        /// The action to execute each iteration. Must produce a value with
        /// `kind: "Continue"` or `kind: "Break"`.
        body: Box<Action>,
    },

    /// Error materialization. Executes the action and reifies success/failure
    /// into `{kind: "Success", value}` or `{kind: "Failure", error, input}`.
    /// Always infallible from the VM's perspective.
    Attempt {
        /// The action to attempt.
        action: Box<Action>,
    },

    /// Named step reference for mutual recursion and DAG topologies.
    Step {
        /// Name of the step to invoke.
        step: String,
    },
}

/// Top-level workflow configuration.
///
/// Pairs a workflow entry point with an optional map of named steps and
/// a read-only context available to all handlers.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Config {
    /// The workflow entry point.
    pub workflow: Action,

    /// Named steps, referenced by [`Action::Step`] nodes.
    #[serde(default)]
    pub steps: HashMap<String, Action>,

    /// Read-only environment passed to all handlers. Carries API keys,
    /// workflow IDs, tenant config, etc.
    #[serde(default = "default_context")]
    pub context: Value,
}

const fn default_context() -> Value {
    Value::Null
}

#[cfg(test)]
#[expect(clippy::unwrap_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_call() {
        let json = r#"{"kind": "Call", "module": "./h.ts", "func": "greet"}"#;
        let action: Action = serde_json::from_str(json).unwrap();
        assert_eq!(
            action,
            Action::Call {
                module: "./h.ts".into(),
                func: "greet".into(),
                step_config: None,
                value_schema: None,
            }
        );
    }

    #[test]
    fn deserialize_sequence() {
        let json = r#"{
            "kind": "Sequence",
            "actions": [
                {"kind": "Call", "module": "./a.ts", "func": "setup"},
                {"kind": "Call", "module": "./b.ts", "func": "run"}
            ]
        }"#;
        let action: Action = serde_json::from_str(json).unwrap();
        let Action::Sequence { actions } = &action else {
            panic!("expected Sequence");
        };
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn deserialize_traverse() {
        let json = r#"{"kind": "Traverse", "action": {"kind": "Call", "module": "./m.ts", "func": "migrate"}}"#;
        let action: Action = serde_json::from_str(json).unwrap();
        assert!(matches!(action, Action::Traverse { .. }));
    }

    #[test]
    fn deserialize_all() {
        let json = r#"{
            "kind": "All",
            "actions": [
                {"kind": "Call", "module": "./a.ts", "func": "one"},
                {"kind": "Call", "module": "./b.ts", "func": "two"}
            ]
        }"#;
        let action: Action = serde_json::from_str(json).unwrap();
        let Action::All { actions } = &action else {
            panic!("expected All");
        };
        assert_eq!(actions.len(), 2);
    }

    #[test]
    fn deserialize_match() {
        let json = r#"{
            "kind": "Match",
            "cases": {
                "HasErrors": {"kind": "Call", "module": "./fix.ts", "func": "fix"},
                "Clean": {"kind": "Call", "module": "./done.ts", "func": "done"}
            }
        }"#;
        let action: Action = serde_json::from_str(json).unwrap();
        let Action::Match { cases } = &action else {
            panic!("expected Match");
        };
        assert_eq!(cases.len(), 2);
        assert!(cases.contains_key("HasErrors"));
        assert!(cases.contains_key("Clean"));
    }

    #[test]
    fn deserialize_loop() {
        let json = r#"{
            "kind": "Loop",
            "body": {
                "kind": "Sequence",
                "actions": [
                    {"kind": "Call", "module": "./check.ts", "func": "check"},
                    {"kind": "Call", "module": "./signal.ts", "func": "recur"}
                ]
            }
        }"#;
        let action: Action = serde_json::from_str(json).unwrap();
        assert!(matches!(action, Action::Loop { .. }));
    }

    #[test]
    fn deserialize_attempt() {
        let json = r#"{
            "kind": "Attempt",
            "action": {"kind": "Call", "module": "./risky.ts", "func": "try_it"}
        }"#;
        let action: Action = serde_json::from_str(json).unwrap();
        assert!(matches!(action, Action::Attempt { .. }));
    }

    #[test]
    fn deserialize_step() {
        let json = r#"{"kind": "Step", "step": "Review"}"#;
        let action: Action = serde_json::from_str(json).unwrap();
        assert_eq!(action, Action::Step { step: "Review".into() });
    }

    #[test]
    fn deserialize_config_minimal() {
        let json = r#"{"workflow": {"kind": "Call", "module": "./h.ts", "func": "run"}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert!(config.steps.is_empty());
    }

    #[test]
    fn deserialize_config_with_steps() {
        let json = r#"{
            "workflow": {"kind": "Step", "step": "Writer"},
            "steps": {
                "Writer": {"kind": "Call", "module": "./w.ts", "func": "write"},
                "Reviewer": {"kind": "Call", "module": "./r.ts", "func": "review"}
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.steps.len(), 2);
    }

    #[test]
    fn deserialize_call_with_step_config() {
        let json = r#"{
            "kind": "Call",
            "module": "./h.ts",
            "func": "run",
            "stepConfig": {"model": "gpt-4"},
            "valueSchema": {"type": "object"}
        }"#;
        let action: Action = serde_json::from_str(json).unwrap();
        let Action::Call { step_config, value_schema, .. } = &action else {
            panic!("expected Call");
        };
        assert!(step_config.is_some());
        assert!(value_schema.is_some());
    }

    #[test]
    fn round_trip_complex_workflow() {
        let json = r#"{
            "workflow": {
                "kind": "Sequence",
                "actions": [
                    {"kind": "Call", "module": "./setup.ts", "func": "setup"},
                    {"kind": "Call", "module": "./list.ts", "func": "listFiles"},
                    {"kind": "Traverse", "action": {"kind": "Call", "module": "./migrate.ts", "func": "migrate"}},
                    {"kind": "Loop", "body": {
                        "kind": "Sequence",
                        "actions": [
                            {"kind": "Call", "module": "./check.ts", "func": "typeCheck"},
                            {"kind": "Call", "module": "./classify.ts", "func": "classify"},
                            {"kind": "Match", "cases": {
                                "HasErrors": {"kind": "Sequence", "actions": [
                                    {"kind": "Call", "module": "./extract.ts", "func": "extractErrors"},
                                    {"kind": "Traverse", "action": {"kind": "Call", "module": "./fix.ts", "func": "fix"}},
                                    {"kind": "Call", "module": "./signal.ts", "func": "recur"}
                                ]},
                                "Clean": {"kind": "Call", "module": "./signal.ts", "func": "done"}
                            }}
                        ]
                    }}
                ]
            }
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();

        // Round-trip through serialize/deserialize
        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: Config = serde_json::from_str(&serialized).unwrap();
        assert_eq!(config, deserialized);
    }
}
