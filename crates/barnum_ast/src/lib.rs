//! Workflow algebra AST types for Barnum.
//!
//! This crate defines the core data model: the [`Action`] enum (a workflow
//! program expressed as a tree of compositional nodes) and the [`Config`]
//! struct (the top-level container wrapping a workflow entry point).
//!
//! TypeScript builds these structures via builder functions and serializes
//! them to JSON. Rust deserializes and interprets them.

pub mod flat;
mod json_schema;

pub use json_schema::JsonSchema;

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use string_key_newtype::string_key_newtype;

// ---------------------------------------------------------------------------
// Interned string newtypes
// ---------------------------------------------------------------------------

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

/// Identifies a resume-style effect handler. Paired with [`ResumePerformAction`].
/// Separate type from [`RestartHandlerId`] prevents cross-matching at compile time.
///
/// `u16` to keep [`FlatEntry`](flat::FlatEntry) at 8 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ResumeHandlerId(pub u16);

impl std::fmt::Display for ResumeHandlerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Identifies a restart-style effect handler. Paired with [`RestartPerformAction`].
/// Separate type from [`ResumeHandlerId`] prevents cross-matching at compile time.
///
/// `u16` to keep [`FlatEntry`](flat::FlatEntry) at 8 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RestartHandlerId(pub u16);

impl std::fmt::Display for RestartHandlerId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

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

    /// Fanout. Passes the same input to all actions, collects results
    /// as an array.
    All(AllAction),

    /// N-ary branch on the `kind` field of a discriminated union input.
    Branch(BranchAction),

    /// Resume-style effect handler. Handler runs inline at the Perform site.
    /// Handler produces `[value, new_state]`. Engine delivers `value` to
    /// the Perform's parent and writes `new_state` back to the handle frame.
    ResumeHandle(ResumeHandleAction),

    /// Raise a resume-style effect. Targets the nearest enclosing
    /// [`ResumeHandle`](Action::ResumeHandle) with matching `resume_handler_id`.
    ResumePerform(ResumePerformAction),

    /// Restart-style effect handler. When `RestartPerform` fires, the body
    /// is torn down immediately and the handler runs. Handler output is the
    /// new body input — the body is re-advanced from scratch.
    RestartHandle(RestartHandleAction),

    /// Raise a restart-style effect. Targets the nearest enclosing
    /// [`RestartHandle`](Action::RestartHandle) with matching `restart_handler_id`.
    RestartPerform(RestartPerformAction),
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

/// Fanout: passes the same input to all actions, collects results as a tuple.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AllAction {
    /// Independent actions to execute.
    pub actions: Vec<Action>,
}

/// N-ary branch on the `kind` field of a discriminated union input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BranchAction {
    /// Map from variant `kind` values to actions.
    pub cases: HashMap<KindDiscriminator, Action>,
}

/// Resume-style effect handler.
///
/// Handler runs inline at the Perform site. Produces `[value, new_state]`.
/// Engine delivers `value` to the Perform's parent and writes `new_state`
/// back to the handle frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumeHandleAction {
    /// Which resume effect type this handler intercepts.
    pub resume_handler_id: ResumeHandlerId,
    /// The action to run (may contain `ResumePerform` nodes).
    pub body: Box<Action>,
    /// The handler DAG invoked when the effect fires.
    pub handler: Box<Action>,
}

/// Raise a resume-style effect. Targets the nearest enclosing
/// [`ResumeHandle`](Action::ResumeHandle) with matching `resume_handler_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResumePerformAction {
    /// Which resume effect type to raise.
    pub resume_handler_id: ResumeHandlerId,
}

/// Restart-style effect handler.
///
/// When `RestartPerform` fires, the body is torn down and the handler runs.
/// Handler output is the new body input — the body re-advances from scratch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartHandleAction {
    /// Which restart effect type this handler intercepts.
    pub restart_handler_id: RestartHandlerId,
    /// The action to run (may contain `RestartPerform` nodes).
    pub body: Box<Action>,
    /// The handler DAG invoked when the effect fires.
    pub handler: Box<Action>,
}

/// Raise a restart-style effect. Targets the nearest enclosing
/// [`RestartHandle`](Action::RestartHandle) with matching `restart_handler_id`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartPerformAction {
    /// Which restart effect type to raise.
    pub restart_handler_id: RestartHandlerId,
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
    /// JSON Schema for the handler's input type, if declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub input_schema: Option<JsonSchema>,
    /// JSON Schema for the handler's output type, if declared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<JsonSchema>,
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
    GetField {
        /// The field name to extract (must be a JSON string).
        value: Value,
    },
    /// Extract an element from an array by index.
    GetIndex {
        /// The zero-based index to extract (must be a JSON number).
        value: Value,
    },
    /// Select named fields from an object, producing a new object with only those fields.
    Pick {
        /// The field names to keep (must be a JSON array of strings).
        value: Value,
    },
    /// Wrap input as `{ kind: "Continue", value: <input> }`.
    TagContinue,
    /// Wrap input as `{ kind: "Break", value: <input> }`.
    TagBreak,
    /// Collect `Some` values from an array of `Option<T>`, discarding `None`s.
    ///
    /// Input: array of `{ kind: "Some", value: T }` or `{ kind: "None", value: _ }`.
    /// Output: array of unwrapped `T` values (only the `Some` entries).
    CollectSome,
    /// Head/tail decomposition of an array.
    ///
    /// Input: array of values.
    /// Output: `{ kind: "Some", value: [first, rest] }` for non-empty arrays,
    ///         `{ kind: "None", value: null }` for empty arrays.
    SplitFirst,
    /// Init/last decomposition of an array.
    ///
    /// Input: array of values.
    /// Output: `{ kind: "Some", value: [init, last] }` for non-empty arrays,
    ///         `{ kind: "None", value: null }` for empty arrays.
    SplitLast,
    /// Wrap input as `{ <field>: <input> }`.
    WrapInField {
        /// The field name (must be a JSON string).
        value: Value,
    },
    /// Sleep for a fixed duration, then pass input through unchanged.
    ///
    /// Unlike other builtins, execution is async — the scheduler awaits
    /// `tokio::time::sleep` before returning the input.
    Sleep {
        /// Duration in milliseconds (must be a non-negative integer).
        value: Value,
    },
}

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Top-level workflow configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Config {
    /// The workflow entry point.
    pub workflow: Action,
}
