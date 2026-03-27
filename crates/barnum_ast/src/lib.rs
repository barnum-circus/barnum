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
use string_key_newtype::string_key_newtype;

// ---------------------------------------------------------------------------
// Interned string newtypes
// ---------------------------------------------------------------------------

string_key_newtype!(
    /// Named step identifier, referenced by [`StepAction`] and [`Config::steps`].
    StepName
);
string_key_newtype!(
    /// Absolute module path to a handler file.
    ModulePath
);
string_key_newtype!(
    /// Exported function name within a handler module.
    FuncName
);
string_key_newtype!(
    /// Value of the `kind` field used to discriminate tagged union variants.
    KindDiscriminator
);

// ---------------------------------------------------------------------------
// Action (the AST)
// ---------------------------------------------------------------------------

/// A single node in the workflow AST.
///
/// Discriminated on `kind` for JSON serialization (`#[serde(tag = "kind")]`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum Action {
    /// Leaf node. Invokes an external handler.
    Invoke(InvokeAction),

    /// Sequential composition. Each action receives the previous action's output.
    Pipe(PipeAction),

    /// Parallel map over an array input. Applies the action to each element.
    ForEach(ForEachAction),

    /// Parallel fanout. Passes the same input to all actions, collects results
    /// as an array.
    Parallel(ParallelAction),

    /// N-ary branch on the `kind` field of a discriminated union input.
    Branch(BranchAction),

    /// Monadic fixed-point iteration. Repeats the body until it signals
    /// `Break`.
    Loop(LoopAction),

    /// Error materialization. Executes the action and reifies success/failure
    /// into `{kind: "Success", value}` or `{kind: "Failure", error, input}`.
    /// Always infallible from the VM's perspective.
    Attempt(AttemptAction),

    /// Named step reference for mutual recursion and DAG topologies.
    Step(StepAction),
}

// ---------------------------------------------------------------------------
// Action variant payloads
// ---------------------------------------------------------------------------

/// Invokes an external handler. The handler type is discriminated by
/// [`HandlerKind`], currently only TypeScript.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InvokeAction {
    /// Which handler to invoke.
    pub handler: HandlerKind,
}

/// Sequential composition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipeAction {
    /// Ordered list of actions to execute.
    pub actions: Vec<Action>,
}

/// Parallel map over an array input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ForEachAction {
    /// The action to apply to each element.
    pub action: Box<Action>,
}

/// Parallel fanout.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParallelAction {
    /// Independent actions to execute in parallel.
    pub actions: Vec<Action>,
}

/// N-ary branch on the `kind` field of a discriminated union input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchAction {
    /// Map from variant `kind` values to actions.
    pub cases: HashMap<KindDiscriminator, Action>,
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

/// Step reference — either a named step or the workflow root.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StepAction {
    /// Which step to jump to.
    pub step: StepRef,
}

/// Target of a step reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum StepRef {
    /// Reference to a named step in [`Config::steps`].
    Named {
        /// The step name.
        name: StepName,
    },
    /// Reference to the workflow entry point (self-recursion).
    Root,
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
    pub module: ModulePath,
    /// Exported function name.
    pub func: FuncName,
    /// Optional per-step configuration schema forwarded to the handler.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_config_schema: Option<Value>,
    /// Optional JSON Schema describing the handler's expected input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_schema: Option<Value>,
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Top-level workflow configuration.
///
/// Pairs a workflow entry point with an optional map of named steps.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// The workflow entry point.
    pub workflow: Action,

    /// Named steps, referenced by [`Action::Step`] nodes.
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub steps: HashMap<StepName, Action>,
}
