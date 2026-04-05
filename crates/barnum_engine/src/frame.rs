//! Frame types for the engine's frame tree.

use barnum_ast::RestartHandlerId;
use barnum_ast::ResumeHandlerId;
use barnum_ast::flat::{ActionId, HandlerId};
use serde_json::Value;

/// Key into the engine's frame arena. Carries a generational index —
/// accessing a removed-and-reused slot with a stale `FrameId` returns `None`.
pub type FrameId = thunderdome::Index;

/// How a child frame refers to its parent.
///
/// One variant per frame kind — the variant determines the code path in
/// `deliver`, eliminating nested `FrameKind` dispatch.
#[derive(Debug, Clone, Copy)]
pub enum ParentRef {
    /// Parent is a Chain frame — sequential, single child.
    Chain {
        /// The parent frame's ID.
        frame_id: FrameId,
    },
    /// Parent is an All frame — indexed child in fan-out.
    All {
        /// The parent frame's ID.
        frame_id: FrameId,
        /// This child's index in the parent's results vector.
        child_index: usize,
    },
    /// Parent is a `ForEach` frame — indexed child per array element.
    ForEach {
        /// The parent frame's ID.
        frame_id: FrameId,
        /// This child's index in the parent's results vector.
        child_index: usize,
    },
    /// Parent is a `ResumeHandle` frame — body child only.
    ResumeHandle {
        /// The parent frame's ID.
        frame_id: FrameId,
    },
    /// Parent is a `ResumePerform` frame — handler child only.
    ResumePerform {
        /// The parent frame's ID.
        frame_id: FrameId,
    },
    /// Parent is a `RestartHandle` frame — either the body or handler side.
    RestartHandle {
        /// The parent frame's ID.
        frame_id: FrameId,
        /// Which side of the `RestartHandle` this child is on.
        side: RestartHandleSide,
    },
}

/// Which side of a `RestartHandle` frame a child belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RestartHandleSide {
    /// The body — the action that may `RestartPerform` effects.
    Body,
    /// The handler — the action invoked when the effect fires.
    Handler,
}

impl ParentRef {
    /// Extract the parent's [`FrameId`] regardless of variant.
    #[must_use]
    pub const fn frame_id(self) -> FrameId {
        match self {
            Self::Chain { frame_id }
            | Self::All { frame_id, .. }
            | Self::ForEach { frame_id, .. }
            | Self::ResumeHandle { frame_id }
            | Self::ResumePerform { frame_id }
            | Self::RestartHandle { frame_id, .. } => frame_id,
        }
    }
}

/// The kind-specific state stored in each frame.
#[derive(Debug)]
pub enum FrameKind {
    /// Sequential: first child active, then trampoline to `rest`.
    Chain {
        /// The remaining action to advance after the first child completes.
        rest: ActionId,
    },
    /// Fan-out: collecting results from N parallel branches.
    All {
        /// Slot per child; `None` until the child completes.
        results: Vec<Option<Value>>,
    },
    /// Fan-out: collecting results from N array elements.
    ForEach {
        /// Slot per element; `None` until the child completes.
        results: Vec<Option<Value>>,
    },
    /// Leaf dispatch: a handler invocation in flight. The `handler`
    /// field duplicates `Dispatch::handler_id` — it's stored here so
    /// the frame tree is self-describing for observability.
    Invoke {
        /// Which handler this task is running.
        handler: HandlerId,
    },
    /// Resume-style effect handler. Handler runs inline at the Perform
    /// site. Never suspends.
    ResumeHandle(ResumeHandleFrame),
    /// Frame at the Perform site for a resume-style effect. The handler
    /// DAG runs as a child of this frame. On handler completion, the
    /// result is destructured as `[value, new_state]`, state is written
    /// back to the `ResumeHandle`, and value is delivered upward.
    ResumePerform(ResumePerformFrame),
    /// Restart-style effect handler. When `RestartPerform` fires, the body
    /// is torn down and the handler runs. Handler output is the new body input.
    RestartHandle(RestartHandleFrame),
    /// Marker for a deferred `RestartPerform`. No data — exists only so
    /// that `teardown_body` can remove it, causing the liveness check to
    /// fail for stale restart effects.
    RestartPerformMarker,
}

/// ResumeHandle-specific state, stored in [`FrameKind::ResumeHandle`].
#[derive(Debug)]
pub struct ResumeHandleFrame {
    /// Which resume effect type this handler intercepts.
    pub resume_handler_id: ResumeHandlerId,
    /// The body action.
    pub body: ActionId,
    /// The handler DAG to invoke when the effect fires.
    pub handler: ActionId,
    /// State value maintained across handler invocations.
    pub state: Value,
}

/// ResumePerform-specific state, stored in [`FrameKind::ResumePerform`].
#[derive(Debug)]
pub struct ResumePerformFrame {
    /// The `ResumeHandle` frame that this Perform targets. Used to write
    /// state back when the handler completes.
    pub resume_handle_frame_id: FrameId,
}

/// RestartHandle-specific state, stored in [`FrameKind::RestartHandle`].
#[derive(Debug)]
pub struct RestartHandleFrame {
    /// Which restart effect type this handler intercepts.
    pub restart_handler_id: RestartHandlerId,
    /// The body action (for re-advancing after handler completes).
    pub body: ActionId,
    /// The handler DAG to invoke when the effect fires.
    pub handler: ActionId,
    /// State value passed to the handler alongside the payload.
    /// Initialized to the `RestartHandle`'s input value. Immutable.
    pub state: Value,
}

/// A single frame in the engine's frame tree.
#[derive(Debug)]
pub struct Frame {
    /// Parent reference. `None` for the top-level action.
    pub parent: Option<ParentRef>,
    /// Kind-specific state.
    pub kind: FrameKind,
}

#[cfg(test)]
mod tests {
    use super::*;
    use barnum_ast::flat::ActionId;
    use thunderdome::Arena;

    /// Removing a frame and reusing its slot must not let the old `FrameId`
    /// resolve — the generational index rejects stale references.
    #[test]
    fn stale_frame_id_returns_none() {
        let mut arena = Arena::<Frame>::new();

        let old_id = arena.insert(Frame {
            parent: None,
            kind: FrameKind::Chain { rest: ActionId(0) },
        });

        arena.remove(old_id);

        // Insert a new frame — thunderdome may reuse the same slot.
        let _new_id = arena.insert(Frame {
            parent: None,
            kind: FrameKind::Chain { rest: ActionId(0) },
        });

        // The old id must not resolve, even if the slot was reused.
        assert!(
            arena.get(old_id).is_none(),
            "stale FrameId must not match a reused slot"
        );
    }
}
