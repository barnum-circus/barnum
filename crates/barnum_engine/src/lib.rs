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

use std::collections::{BTreeMap, VecDeque};

use barnum_ast::EffectId;
use barnum_ast::HandlerKind;
use barnum_ast::ResumeHandlerId;
use barnum_ast::flat::{ActionId, FlatConfig, HandlerId};
use frame::{Frame, FrameId, ParentRef};
use serde::Deserialize;
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
    /// A `Perform` node was reached but no enclosing `Handle` matches
    /// the effect type.
    #[error("unhandled effect: {effect_id}")]
    UnhandledEffect {
        /// The effect type that was not handled.
        effect_id: EffectId,
    },
    /// A `ResumePerform` node was reached but no enclosing `ResumeHandle`
    /// matches the resume handler ID.
    #[error("unhandled resume effect: {resume_handler_id}")]
    UnhandledResumeEffect {
        /// The resume handler ID that was not handled.
        resume_handler_id: ResumeHandlerId,
    },
}

// ---------------------------------------------------------------------------
// CompleteError
// ---------------------------------------------------------------------------

/// Errors that can occur during [`WorkflowState::complete`].
#[derive(Debug, thiserror::Error)]
pub enum CompleteError {
    /// An advance error occurred during Chain trampoline or Handle re-entry.
    #[error(transparent)]
    Advance(#[from] AdvanceError),
    /// A handler returned a value that doesn't deserialize as a valid
    /// [`HandlerOutput`].
    #[error("invalid handler output: {source}")]
    InvalidHandlerOutput {
        /// The serde deserialization error.
        #[from]
        source: serde_json::Error,
    },
}

// ---------------------------------------------------------------------------
// HandlerOutput / StateUpdate (serde types)
// ---------------------------------------------------------------------------

/// The output of a handler DAG, deserialized from the handler's return value.
#[derive(Debug, Deserialize)]
#[serde(tag = "kind")]
enum HandlerOutput {
    /// Resume the body at the Perform site with the given value.
    Resume {
        /// The value to deliver to the Perform's parent.
        value: Value,
        /// Whether to update the Handle's state.
        #[serde(default)]
        state_update: StateUpdate,
    },
    /// Tear down the body and re-advance from the Handle's body action.
    RestartBody {
        /// The input value for the re-advanced body.
        value: Value,
        /// Whether to update the Handle's state.
        #[serde(default)]
        state_update: StateUpdate,
    },
}

/// Whether a handler updated the Handle's state.
#[derive(Debug, Default, Deserialize)]
#[serde(tag = "kind")]
enum StateUpdate {
    /// State unchanged.
    #[default]
    Unchanged,
    /// Replace the state with a new value.
    Updated {
        /// The new state value.
        value: Value,
    },
}

// ---------------------------------------------------------------------------
// StashedItem / engine enums
// ---------------------------------------------------------------------------

/// A deferred work item waiting to be processed.
#[derive(Debug)]
enum StashedItem {
    /// A task completion waiting to be delivered.
    Delivery {
        /// Where to deliver the value.
        parent_ref: ParentRef,
        /// The value to deliver.
        value: Value,
    },
    /// An effect waiting to be bubbled.
    Effect {
        /// The Perform's parent — where bubbling starts.
        starting_parent: ParentRef,
        /// Which effect type.
        effect_id: EffectId,
        /// The Perform's input value (handler payload).
        payload: Value,
    },
}

/// Result of checking the ancestor chain for blockers.
#[derive(Debug)]
enum AncestorCheck {
    /// No blockers — the path to the root (or target Handle) is clear.
    Clear,
    /// A suspended Handle blocks the path.
    Blocked,
    /// A frame in the ancestor chain was torn down (generational arena miss).
    FrameGone,
}

/// Result of `try_deliver` — does NOT mutate the stash.
#[derive(Debug)]
enum TryDeliverResult {
    /// Delivery succeeded. Inner value is the workflow result, if any.
    Delivered(Option<Value>),
    /// Target frame is blocked by a suspended Handle. Returns the value
    /// so the caller can stash without cloning.
    Blocked(Value),
    /// Target frame was torn down.
    FrameGone,
}

/// Result of `bubble_effect` — does NOT mutate the stash.
#[derive(Debug)]
enum StashOutcome {
    /// Effect was dispatched to a handler, or dropped (frame gone).
    Consumed,
    /// Target is blocked by a suspended Handle. Returns the payload
    /// so the caller can stash without cloning.
    Blocked(Value),
}

/// Result of a single sweep pass over the stash.
#[derive(Debug)]
enum SweepResult {
    /// The workflow produced a terminal value.
    WorkflowDone(Value),
    /// At least one item was consumed — tree state may have changed.
    MadeProgress,
    /// No items were consumed — all remain blocked or stash was empty.
    NoProgress,
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
    stashed_items: VecDeque<StashedItem>,
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
            stashed_items: VecDeque::new(),
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
