//! Pure state-machine workflow engine for Barnum.
//!
//! The engine is a synchronous state machine with no I/O, no async, no timers,
//! and no concurrency. External code drives it by calling [`advance::advance`]
//! and popping effects via [`WorkflowState::pop_pending_effect`].

/// Advance (expand) an action into frames.
pub mod advance;
mod ancestors;
/// Deliver a completed task result back to the workflow.
pub mod complete;
/// Effect processing: restart and resume handlers.
pub mod effects;
pub mod frame;
#[cfg(test)]
pub(crate) mod test_helpers;

use std::collections::BTreeMap;
use std::collections::VecDeque;

use barnum_ast::HandlerKind;
use barnum_ast::RestartHandlerId;
use barnum_ast::ResumeHandlerId;
use barnum_ast::flat::{ActionId, FlatConfig, HandlerId};
use frame::{Frame, FrameId};
use serde_json::Value;
use thunderdome::Arena;
use u32_newtype::u32_newtype;

// ---------------------------------------------------------------------------
// TaskId
// ---------------------------------------------------------------------------

u32_newtype!(
    /// Identifies a pending handler invocation. Assigned by the engine,
    /// returned to the engine in [`complete::complete`].
    TaskId
);

// ---------------------------------------------------------------------------
// DispatchEvent
// ---------------------------------------------------------------------------

/// A pending handler invocation produced by [`advance::advance`].
#[derive(Debug)]
pub struct DispatchEvent {
    /// Correlates this dispatch with the result delivered to
    /// [`complete::complete`].
    pub task_id: TaskId,
    /// Index into the handler pool. Resolve via [`WorkflowState::handler`].
    pub handler_id: HandlerId,
    /// The value to pass to the handler.
    pub value: Value,
}

// ---------------------------------------------------------------------------
// CompletionEvent
// ---------------------------------------------------------------------------

/// A completed handler result, ready to be delivered to the workflow engine
/// via [`complete::complete`].
#[derive(Debug)]
pub struct CompletionEvent {
    /// The task that completed.
    pub task_id: TaskId,
    /// The handler's return value.
    pub value: Value,
}

// ---------------------------------------------------------------------------
// PendingEffect
// ---------------------------------------------------------------------------

/// `(FrameId, PendingEffectKind)` — the liveness key and effect payload.
pub type PendingEffect = (FrameId, PendingEffectKind);

/// The payload of a pending effect.
#[derive(Debug)]
pub enum PendingEffectKind {
    /// A handler invocation ready to be dispatched to a worker.
    Dispatch(DispatchEvent),
    /// A deferred restart. The body will be torn down and the handler advanced.
    Restart(RestartEvent),
}

/// A deferred restart effect. The `FrameId` in the `PendingEffect` tuple
/// is the marker frame (liveness key). This struct carries the handle
/// target and payload.
#[derive(Debug)]
pub struct RestartEvent {
    /// The `RestartHandle` frame that will process this restart.
    pub restart_handle_frame_id: FrameId,
    /// The payload value passed to the handler.
    pub payload: Value,
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

/// Errors that can occur during [`complete::complete`].
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
/// Call [`advance::advance`] with [`workflow_root`](WorkflowState::workflow_root)
/// to begin execution, then pop effects via
/// [`pop_pending_effect`](WorkflowState::pop_pending_effect). Deliver handler
/// results via [`complete::complete`].
#[derive(Debug)]
pub struct WorkflowState {
    flat_config: FlatConfig,
    frames: Arena<Frame>,
    task_to_frame: BTreeMap<TaskId, FrameId>,
    pending_effects: VecDeque<PendingEffect>,
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
            pending_effects: VecDeque::new(),
            next_task_id: 0,
        }
    }

    /// The underlying flat configuration.
    #[must_use]
    pub const fn flat_config(&self) -> &FlatConfig {
        &self.flat_config
    }

    /// The workflow's root action. Pass this to [`advance`](WorkflowState::advance)
    /// with the initial input to start execution.
    #[must_use]
    pub const fn workflow_root(&self) -> ActionId {
        self.flat_config.workflow_root()
    }

    /// Pop the next pending effect, or `None` if the queue is empty.
    pub fn pop_pending_effect(&mut self) -> Option<PendingEffect> {
        self.pending_effects.pop_front()
    }

    /// Returns true if `frame_id` still exists in the frame arena.
    /// The single liveness check for all event types.
    #[must_use]
    pub fn is_frame_live(&self, frame_id: FrameId) -> bool {
        self.frames.contains(frame_id)
    }

    /// Look up the Invoke frame ID for a task. Returns `None` if the task
    /// was torn down (stale completion from the scheduler).
    #[must_use]
    pub fn task_frame_id(&self, task_id: TaskId) -> Option<FrameId> {
        self.task_to_frame.get(&task_id).copied()
    }

    /// Look up a handler by ID. Used by the caller to resolve
    /// [`DispatchEvent::handler_id`].
    #[must_use]
    pub fn handler(&self, id: HandlerId) -> &HandlerKind {
        self.flat_config.handler(id)
    }

    /// Look up the `HandlerId` for a live task. Reads the Invoke frame
    /// without removing it — safe to call before [`complete::complete`].
    ///
    /// # Panics
    ///
    /// Panics if `task_id` is unknown or its frame is not an Invoke.
    #[must_use]
    #[allow(clippy::expect_used, clippy::panic)]
    pub fn handler_id_for_task(&self, task_id: TaskId) -> HandlerId {
        let frame_id = self
            .task_to_frame
            .get(&task_id)
            .expect("handler_id_for_task: unknown task");
        let frame = self
            .frames
            .get(*frame_id)
            .expect("handler_id_for_task: frame not in arena");
        match frame.kind {
            frame::FrameKind::Invoke { handler } => handler,
            ref other => panic!("handler_id_for_task: expected Invoke frame, got {other:?}"),
        }
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
