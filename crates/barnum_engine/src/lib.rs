//! Pure state-machine workflow engine for Barnum.
//!
//! The engine is a synchronous state machine with no I/O, no async, no timers,
//! and no concurrency. External code drives it by calling [`WorkflowState::advance`]
//! and draining dispatches via [`WorkflowState::take_pending_dispatches`].

mod advance;
mod ancestors;
mod complete;
mod effects;
pub mod frame;
#[cfg(test)]
pub(crate) mod test_helpers;

use std::collections::BTreeMap;

use barnum_ast::HandlerKind;
use barnum_ast::RestartHandlerId;
use barnum_ast::ResumeHandlerId;
use barnum_ast::flat::{ActionId, FlatConfig, HandlerId};
use frame::{Frame, FrameId, ParentRef};
use serde_json::Value;
use thunderdome::Arena;
use u32_newtype::u32_newtype;

// ---------------------------------------------------------------------------
// TaskId
// ---------------------------------------------------------------------------

u32_newtype!(
    /// Identifies a pending handler invocation. Assigned by the engine,
    /// returned to the engine in [`WorkflowState::complete`].
    TaskId
);

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// A pending handler invocation produced by `advance`.
#[derive(Debug)]
pub struct Dispatch {
    /// Correlates this dispatch with the result delivered to
    /// [`WorkflowState::complete`].
    pub task_id: TaskId,
    /// Index into the handler pool. Resolve via [`WorkflowState::handler`].
    pub handler_id: HandlerId,
    /// The value to pass to the handler.
    pub value: Value,
}

// ---------------------------------------------------------------------------
// AdvanceError
// ---------------------------------------------------------------------------

/// Errors that can occur during advance.
#[derive(Debug, thiserror::Error)]
pub enum AdvanceError {
    /// `ForEach` received a non-array input.
    #[error("ForEach expected array input, got: {value}")]
    ForEachExpectedArray {
        /// The non-array value that was received.
        value: Value,
    },
    /// `Branch` input lacks a string `kind` field.
    #[error("Branch input must have a string 'kind' field, got: {value}")]
    BranchMissingKind {
        /// The input value missing the `kind` field.
        value: Value,
    },
    /// No branch case matched the input's `kind` value.
    #[error("no matching branch case for kind {kind:?}")]
    BranchNoMatch {
        /// The `kind` value that had no matching case.
        kind: String,
    },
    /// A `ResumePerform` node was reached but no enclosing `ResumeHandle`
    /// matches the resume handler ID.
    #[error("unhandled resume effect: {resume_handler_id}")]
    UnhandledResumeEffect {
        /// The resume handler ID that was not handled.
        resume_handler_id: ResumeHandlerId,
    },
    /// A `RestartPerform` node was reached but no enclosing `RestartHandle`
    /// matches the restart handler ID.
    #[error("unhandled restart effect: {restart_handler_id}")]
    UnhandledRestartEffect {
        /// The restart handler ID that was not handled.
        restart_handler_id: RestartHandlerId,
    },
}

// ---------------------------------------------------------------------------
// CompleteError
// ---------------------------------------------------------------------------

/// Errors that can occur during [`WorkflowState::complete`].
#[derive(Debug, thiserror::Error)]
pub enum CompleteError {
    /// An advance error occurred during Chain trampoline or `RestartHandle`
    /// re-entry.
    #[error(transparent)]
    Advance(#[from] AdvanceError),
    /// A resume handler returned a value that doesn't deserialize as
    /// `[value, new_state]`.
    #[error("invalid handler output: {source}")]
    InvalidHandlerOutput {
        /// The serde deserialization error.
        #[from]
        source: serde_json::Error,
    },
}

// ---------------------------------------------------------------------------
// Engine
// ---------------------------------------------------------------------------

/// Pure state-machine workflow engine.
///
/// Call [`advance`](WorkflowState::advance) with [`workflow_root`](WorkflowState::workflow_root)
/// to begin execution, then drain dispatches via
/// [`take_pending_dispatches`](WorkflowState::take_pending_dispatches). Deliver handler
/// results via [`complete`](WorkflowState::complete).
#[derive(Debug)]
pub struct WorkflowState {
    flat_config: FlatConfig,
    frames: Arena<Frame>,
    task_to_frame: BTreeMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    next_task_id: u32,
}

impl WorkflowState {
    /// Create a new engine from a flattened config.
    #[must_use]
    #[allow(clippy::missing_const_for_fn)] // BTreeMap::new() is not const
    pub fn new(flat_config: FlatConfig) -> Self {
        Self {
            flat_config,
            frames: Arena::new(),
            task_to_frame: BTreeMap::new(),
            pending_dispatches: Vec::new(),
            next_task_id: 0,
        }
    }

    /// The workflow's root action. Pass this to [`advance`](WorkflowState::advance)
    /// with the initial input to start execution.
    #[must_use]
    pub const fn workflow_root(&self) -> ActionId {
        self.flat_config.workflow_root()
    }

    /// Drain all pending dispatches accumulated since the last call.
    pub fn take_pending_dispatches(&mut self) -> Vec<Dispatch> {
        std::mem::take(&mut self.pending_dispatches)
    }

    /// Look up a handler by ID. Used by the caller to resolve
    /// [`Dispatch::handler_id`].
    #[must_use]
    pub fn handler(&self, id: HandlerId) -> &HandlerKind {
        self.flat_config.handler(id)
    }

    /// Expand an `ActionId` into frames. Creates frames for structural
    /// combinators and Invoke leaves.
    ///
    /// Pass `parent: None` for the top-level action (i.e., starting a
    /// workflow). Internal recursion provides `Some(parent_ref)` to attach
    /// child frames to their parent.
    ///
    /// # Errors
    ///
    /// Returns [`AdvanceError`] if the workflow encounters a structural error
    /// during expansion (e.g., `ForEach` on a non-array, `Branch` with no
    /// matching case).
    pub fn advance(
        &mut self,
        action_id: ActionId,
        value: Value,
        parent: Option<ParentRef>,
    ) -> Result<(), AdvanceError> {
        advance::advance(self, action_id, value, parent)
    }

    /// Deliver a task result. The caller invokes this when a dispatched
    /// handler finishes.
    ///
    /// Returns `Ok(Some(value))` when the workflow terminates, `Ok(None)`
    /// when it's still running.
    ///
    /// # Errors
    ///
    /// Returns [`CompleteError`] if the result value has an invalid shape,
    /// or if an advance error occurs during Chain trampoline or Handle re-entry.
    ///
    /// # Panics
    ///
    /// Panics if `task_id` is not a known pending task.
    pub fn complete(
        &mut self,
        task_id: TaskId,
        value: Value,
    ) -> Result<Option<Value>, CompleteError> {
        complete::complete(self, task_id, value)
    }

    // -- Private helpers --

    fn insert_frame(&mut self, frame: Frame) -> FrameId {
        self.frames.insert(frame)
    }

    #[allow(clippy::missing_const_for_fn)] // mutates self
    fn next_task_id(&mut self) -> TaskId {
        let id = TaskId(self.next_task_id);
        self.next_task_id += 1;
        id
    }
}
