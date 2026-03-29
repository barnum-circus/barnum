//! Workflow algebra AST types for Barnum.
//!
//! This crate defines the core data model: the [`Action`] enum (a workflow
//! program expressed as a tree of compositional nodes) and the [`Config`]
//! struct (the top-level container that pairs a workflow entry point with
//! named steps for mutual recursion).
//!
//! TypeScript builds these structures via builder functions and serializes
//! them to JSON. Rust deserializes and interprets them.

pub mod flat;

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

    /// Binary sequential composition. Runs `first`, feeds its output to `rest`.
    Chain(ChainAction),

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

/// Binary sequential composition: run `first`, feed its output to `rest`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainAction {
    /// The action to run first.
    pub first: Box<Action>,
    /// The action to run with `first`'s output.
    pub rest: Box<Action>,
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

/// Discriminated union of handler types.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum HandlerKind {
    /// Run a TypeScript handler file as a subprocess.
    TypeScript(TypeScriptHandler),
    /// Execute a builtin operation inline (no subprocess).
    Builtin(BuiltinHandler),
}

/// A TypeScript handler: module path + exported function name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TypeScriptHandler {
    /// Module path (absolute — JS layer resolves before passing to Rust).
    pub module: ModulePath,
    /// Exported function name.
    pub func: FuncName,
}

/// A builtin handler: wraps a [`BuiltinKind`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinHandler {
    /// Which builtin operation to execute.
    pub builtin: BuiltinKind,
}

/// Discriminated union of builtin operations.
///
/// Builtins are executed inline by the scheduler (no subprocess). Each
/// variant corresponds to a pure data transformation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum BuiltinKind {
    /// Return a fixed value, ignoring input.
    Constant {
        /// The value to return.
        value: Value,
    },
    /// Return the input unchanged.
    Identity,
    /// Discard input, return null.
    Drop,
    /// Wrap input as `{ kind: <value>, value: <input> }`.
    Tag {
        /// The tag string (e.g. `"Continue"`, `"Break"`).
        value: Value,
    },
    /// Merge an array of objects into a single object.
    Merge,
    /// Flatten a nested array one level.
    Flatten,
    /// Extract a named field from an object.
    ExtractField {
        /// The field name to extract (must be a JSON string).
        value: Value,
    },
    /// Extract an element from an array by index.
    ExtractIndex {
        /// The zero-based index to extract (must be a JSON number).
        value: Value,
    },
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
