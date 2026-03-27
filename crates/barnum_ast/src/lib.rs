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

// ---------------------------------------------------------------------------
// Action (the AST)
// ---------------------------------------------------------------------------

/// A single node in the workflow AST.
///
/// Discriminated on `kind` for JSON serialization (`#[serde(tag = "kind")]`).
/// See `refactors/pending/WORKFLOW_ALGEBRA.md` for the full specification of
/// each variant's semantics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Action {
    /// Leaf node. Invokes an external handler.
    Call(CallAction),

    /// Sequential composition. Each action receives the previous action's output.
    Sequence(SequenceAction),

    /// Parallel map over an array input. Applies the action to each element.
    Traverse(TraverseAction),

    /// Parallel fanout. Passes the same input to all actions, collects results
    /// as an array.
    All(AllAction),

    /// N-ary branch on the `kind` field of a discriminated union input.
    Match(MatchAction),

    /// Monadic fixed-point iteration. Repeats the body until it signals
    /// `Break`.
    Loop(LoopAction),

    /// Error materialization. Executes the action and reifies success/failure
    /// into `{kind: "Success", value}` or `{kind: "Failure", error, input}`.
    /// Always infallible from the VM's perspective.
    Attempt(AttemptAction),

    /// Rust-native data transformation. Executes entirely in the VM without
    /// FFI.
    Builtin(BuiltinAction),

    /// Named step reference for mutual recursion and DAG topologies.
    Step(StepAction),
}

// ---------------------------------------------------------------------------
// Action variant payloads
// ---------------------------------------------------------------------------

/// Invokes an external handler. The handler type is discriminated by
/// [`HandlerKind`], currently only TypeScript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CallAction {
    /// Which handler to invoke.
    pub handler: HandlerKind,
}

/// Sequential composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SequenceAction {
    /// Ordered list of actions to execute.
    pub actions: Vec<Action>,
}

/// Parallel map over an array input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TraverseAction {
    /// The action to apply to each element.
    pub action: Box<Action>,
}

/// Parallel fanout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllAction {
    /// Independent actions to execute in parallel.
    pub actions: Vec<Action>,
}

/// N-ary branch on the `kind` field of a discriminated union input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MatchAction {
    /// Map from variant `kind` values to actions.
    pub cases: HashMap<String, Action>,
}

/// Monadic fixed-point iteration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopAction {
    /// The action to execute each iteration. Must produce a value with
    /// `kind: "Continue"` or `kind: "Break"`.
    pub body: Box<Action>,
}

/// Error materialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttemptAction {
    /// The action to attempt.
    pub action: Box<Action>,
}

/// Rust-native data transformation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinAction {
    /// The specific operation to perform.
    pub op: BuiltinOp,
}

/// Named step reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepAction {
    /// Name of the step to invoke.
    pub step: String,
}

// ---------------------------------------------------------------------------
// HandlerKind
// ---------------------------------------------------------------------------

/// Discriminated union of handler types. Currently only TypeScript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum HandlerKind {
    /// Run a TypeScript handler file as a subprocess.
    TypeScript(TypeScriptHandler),
}

/// A TypeScript handler: module path + exported function name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TypeScriptHandler {
    /// Module path (absolute — JS layer resolves before passing to Rust).
    pub module: String,
    /// Exported function name.
    pub func: String,
    /// Optional per-step configuration forwarded to the handler.
    #[serde(default)]
    pub step_config: Option<Value>,
    /// Optional JSON Schema describing the handler's expected input.
    #[serde(default)]
    pub value_schema: Option<Value>,
}

// ---------------------------------------------------------------------------
// BuiltinOp
// ---------------------------------------------------------------------------

/// A Rust-native operation executed without FFI overhead.
///
/// These are the "structural glue" operations that shape data between
/// handler calls. The TypeScript builder provides semantic aliases
/// (e.g., `recur()` compiles to `Tag { kind: "Continue" }`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum BuiltinOp {
    /// Wraps input as `{ kind, value: input }`. Used for loop signals
    /// (`recur()` = Tag "Continue", `done()` = Tag "Break") and any
    /// discriminated union construction.
    Tag(TagOp),

    /// Passes input through unchanged.
    Identity,

    /// Merges an array of objects into a single object.
    /// `[{a: 1}, {b: 2}]` becomes `{a: 1, b: 2}`.
    Merge,

    /// Flattens a nested array one level.
    /// `[[1, 2], [3]]` becomes `[1, 2, 3]`.
    Flatten,

    /// Extracts a single field from an object.
    /// `{a: 1, b: 2}` with field "a" becomes `1`.
    ExtractField(ExtractFieldOp),
}

/// Wraps input as `{ kind, value: input }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TagOp {
    /// The `kind` value to tag with.
    pub kind: String,
}

/// Extracts a single field from an object.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExtractFieldOp {
    /// The field name to extract.
    pub field: String,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Top-level workflow configuration.
///
/// Pairs a workflow entry point with an optional map of named steps and
/// a read-only context available to all handlers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
#[expect(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// Exercises every AST variant in a single workflow. Verifies JSON
    /// round-trip fidelity.
    #[test]
    fn round_trip_full_workflow() {
        let json = r#"{
            "workflow": {
                "kind": "Sequence",
                "actions": [
                    {"kind": "Call", "handler": {"kind": "TypeScript", "module": "./setup.ts", "func": "setup", "stepConfig": {"model": "gpt-4"}}},
                    {"kind": "All", "actions": [
                        {"kind": "Call", "handler": {"kind": "TypeScript", "module": "./list.ts", "func": "listFiles"}},
                        {"kind": "Builtin", "op": {"type": "Identity"}}
                    ]},
                    {"kind": "Builtin", "op": {"type": "Merge"}},
                    {"kind": "Traverse", "action": {"kind": "Call", "handler": {"kind": "TypeScript", "module": "./migrate.ts", "func": "migrate"}}},
                    {"kind": "Builtin", "op": {"type": "Flatten"}},
                    {"kind": "Builtin", "op": {"type": "ExtractField", "field": "errors"}},
                    {"kind": "Attempt", "action": {"kind": "Call", "handler": {"kind": "TypeScript", "module": "./risky.ts", "func": "try_it"}}},
                    {"kind": "Match", "cases": {
                        "Success": {"kind": "Step", "step": "Process"},
                        "Failure": {"kind": "Builtin", "op": {"type": "Tag", "kind": "Break"}}
                    }},
                    {"kind": "Loop", "body": {
                        "kind": "Sequence",
                        "actions": [
                            {"kind": "Call", "handler": {"kind": "TypeScript", "module": "./check.ts", "func": "typeCheck"}},
                            {"kind": "Builtin", "op": {"type": "Tag", "kind": "Continue"}}
                        ]
                    }}
                ]
            },
            "steps": {
                "Process": {"kind": "Call", "handler": {"kind": "TypeScript", "module": "./process.ts", "func": "run"}}
            },
            "context": {"apiKey": "sk-test"}
        }"#;
        let config: Config = serde_json::from_str(json).unwrap();

        // Verify structural properties
        assert_eq!(config.steps.len(), 1);
        assert!(config.steps.contains_key("Process"));
        assert_eq!(config.context, serde_json::json!({"apiKey": "sk-test"}));

        // Round-trip through serialize/deserialize
        let serialized = serde_json::to_string(&config).unwrap();
        let deserialized: Config = serde_json::from_str(&serialized).unwrap();
        assert_eq!(config, deserialized);
    }
}
