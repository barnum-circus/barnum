//! Pure state-machine workflow engine for Barnum.
//!
//! The engine is a synchronous state machine with no I/O, no async, no timers,
//! and no concurrency. External code drives it by calling [`WorkflowState::advance`]
//! and draining dispatches via [`WorkflowState::take_pending_dispatches`].

pub mod frame;

use std::collections::{BTreeMap, VecDeque};

use barnum_ast::EffectId;
use barnum_ast::HandlerKind;
use barnum_ast::flat::{ActionId, FlatAction, FlatConfig, HandlerId};
use frame::{Frame, FrameId, FrameKind, HandleFrame, HandleSide, HandleStatus, ParentRef};
use intern::Lookup;
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
// Ancestors iterator
// ---------------------------------------------------------------------------

/// Walks up the frame tree from a starting [`ParentRef`].
///
/// Yields `(ParentRef, &Frame)` for each ancestor. The [`ParentRef`] is
/// the edge from the child to this frame — the same value that was used to
/// look up the frame. [`FrameId`] is extractable via
/// [`ParentRef::frame_id()`].
///
/// Iteration stops when:
/// - A frame's `parent` is `None` (reached the root). The root frame
///   itself IS yielded; iteration stops after it.
/// - A [`FrameId`] resolves to `None` in the arena (frame was removed).
///   The gone frame is NOT yielded.
struct Ancestors<'a> {
    frames: &'a Arena<Frame>,
    next: Option<ParentRef>,
}

impl<'a> Iterator for Ancestors<'a> {
    type Item = (ParentRef, &'a Frame);

    fn next(&mut self) -> Option<Self::Item> {
        let parent_ref = self.next.take()?;
        let frame = self.frames.get(parent_ref.frame_id())?;
        self.next = frame.parent;
        Some((parent_ref, frame))
    }
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

    /// Walk ancestors starting from `parent_ref`.
    ///
    /// See [`Ancestors`] for iteration semantics.
    const fn ancestors(&self, parent_ref: ParentRef) -> Ancestors<'_> {
        Ancestors {
            frames: &self.frames,
            next: Some(parent_ref),
        }
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
    #[allow(clippy::expect_used)]
    pub fn complete(
        &mut self,
        task_id: TaskId,
        value: Value,
    ) -> Result<Option<Value>, CompleteError> {
        let frame_id = self.task_to_frame.remove(&task_id).expect("unknown task");
        let frame = self.frames.remove(frame_id).expect("invoke frame exists");
        debug_assert!(
            matches!(frame.kind, FrameKind::Invoke { .. }),
            "task_to_frame pointed at non-Invoke frame: {:?}",
            frame.kind,
        );
        let result = match frame.parent {
            Some(parent_ref) => match self.try_deliver(parent_ref, value)? {
                TryDeliverResult::Delivered(result) => result,
                TryDeliverResult::Blocked(value) => {
                    self.stashed_items
                        .push_back(StashedItem::Delivery { parent_ref, value });
                    None
                }
                TryDeliverResult::FrameGone => None,
            },
            None => Some(value),
        };
        if result.is_some() {
            return Ok(result);
        }
        self.sweep_stash()
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

    // -- Effect infrastructure --

    /// Check whether a `ParentRef`'s path to the root is blocked by a
    /// suspended Handle, or whether the frame has been torn down.
    ///
    /// Walks from `parent_ref` up the parent chain. At each edge, checks
    /// whether the edge crosses from a body child into a suspended Handle.
    fn find_blocking_ancestor(&self, parent_ref: ParentRef) -> AncestorCheck {
        // If the immediate parent is gone, short-circuit. Once a frame is
        // present, the entire ancestor chain up to root is guaranteed present
        // (teardown removes descendants before parents).
        if self.frames.get(parent_ref.frame_id()).is_none() {
            return AncestorCheck::FrameGone;
        }
        for (edge, frame) in self.ancestors(parent_ref) {
            if Self::is_blocked_by_handle(&edge, &frame.kind) {
                return AncestorCheck::Blocked;
            }
        }
        AncestorCheck::Clear
    }

    /// Does this parent edge cross from a body child into a suspended Handle?
    const fn is_blocked_by_handle(parent_ref: &ParentRef, parent_kind: &FrameKind) -> bool {
        if let ParentRef::Handle {
            side: HandleSide::Body,
            ..
        } = parent_ref
            && let FrameKind::Handle(handle_frame) = parent_kind
        {
            return matches!(handle_frame.status, HandleStatus::Suspended(_));
        }
        false
    }

    /// Check if delivery is possible and deliver if so. Does NOT push
    /// to the stash — the caller is responsible for stashing on `Blocked`.
    fn try_deliver(
        &mut self,
        parent_ref: ParentRef,
        value: Value,
    ) -> Result<TryDeliverResult, CompleteError> {
        match self.find_blocking_ancestor(parent_ref) {
            AncestorCheck::FrameGone => Ok(TryDeliverResult::FrameGone),
            AncestorCheck::Blocked => Ok(TryDeliverResult::Blocked(value)),
            AncestorCheck::Clear => {
                let result = self.deliver(Some(parent_ref), value)?;
                Ok(TryDeliverResult::Delivered(result))
            }
        }
    }

    /// Walk the parent chain from `starting_parent` upward looking for a
    /// Handle that matches `effect_id`. If found, dispatch to it. Does NOT
    /// mutate the stash.
    #[allow(clippy::expect_used)]
    fn bubble_effect(
        &mut self,
        starting_parent: ParentRef,
        effect_id: EffectId,
        payload: Value,
    ) -> Result<StashOutcome, AdvanceError> {
        // First: can this effect proceed at all?
        match self.find_blocking_ancestor(starting_parent) {
            AncestorCheck::FrameGone => return Ok(StashOutcome::Consumed),
            AncestorCheck::Blocked => return Ok(StashOutcome::Blocked(payload)),
            AncestorCheck::Clear => {}
        }

        // Not blocked — walk the parent chain to find the matching Handle.
        self.find_and_dispatch_handler(starting_parent, effect_id, payload)
    }

    /// Walk from `starting_parent` upward. All ancestors are guaranteed
    /// present and unblocked (caller checked via `find_blocking_ancestor`).
    fn find_and_dispatch_handler(
        &mut self,
        starting_parent: ParentRef,
        effect_id: EffectId,
        payload: Value,
    ) -> Result<StashOutcome, AdvanceError> {
        let handle_frame_id = self
            .ancestors(starting_parent)
            .find_map(|(edge, frame)| {
                if let FrameKind::Handle(handle_frame) = &frame.kind
                    && handle_frame.effect_id == effect_id
                {
                    Some(edge.frame_id())
                } else {
                    None
                }
            })
            .ok_or(AdvanceError::UnhandledEffect { effect_id })?;

        self.dispatch_to_handler(handle_frame_id, starting_parent, payload)?;
        Ok(StashOutcome::Consumed)
    }

    /// Dispatch a handler for a matched effect. Suspends the Handle and
    /// advances the handler DAG.
    #[allow(clippy::expect_used, clippy::needless_pass_by_value)]
    fn dispatch_to_handler(
        &mut self,
        handle_frame_id: FrameId,
        perform_parent: ParentRef,
        payload: Value,
    ) -> Result<(), AdvanceError> {
        // Look up the handler ActionId while we have immutable access.
        let handle_frame = self
            .frames
            .get(handle_frame_id)
            .expect("Handle frame exists");
        let FrameKind::Handle(ref handle) = handle_frame.kind else {
            unreachable!("dispatch_to_handler called on non-Handle frame");
        };
        let handler_action_id = handle.handler;
        let state = handle.state.clone();

        // Assert the Handle is free before suspending.
        assert!(
            matches!(handle.status, HandleStatus::Free),
            "dispatch_to_handler: Handle must be Free, got {:?}",
            handle.status,
        );

        // Mark the Handle as suspended.
        let handle_frame = self
            .frames
            .get_mut(handle_frame_id)
            .expect("Handle frame exists");
        let FrameKind::Handle(ref mut handle) = handle_frame.kind else {
            unreachable!();
        };
        handle.status = HandleStatus::Suspended(perform_parent);

        // Build the handler input: [payload, state].
        let handler_input = serde_json::json!([payload, state]);

        // Advance the handler DAG.
        self.advance(
            handler_action_id,
            handler_input,
            Some(ParentRef::Handle {
                frame_id: handle_frame_id,
                side: HandleSide::Handler,
            }),
        )?;

        Ok(())
    }

    /// Process the handler's completion value.
    #[allow(clippy::expect_used)]
    fn handle_handler_completion(
        &mut self,
        handle_frame_id: FrameId,
        handler_value: Value,
    ) -> Result<Option<Value>, CompleteError> {
        let handler_output: HandlerOutput = serde_json::from_value(handler_value)?;

        match handler_output {
            HandlerOutput::Resume {
                value,
                state_update,
            } => {
                self.apply_state_update(handle_frame_id, state_update);
                self.resume_continuation(handle_frame_id, value)
            }
            HandlerOutput::RestartBody {
                value,
                state_update,
            } => {
                self.apply_state_update(handle_frame_id, state_update);
                self.restart_body(handle_frame_id, value)
            }
        }
    }

    /// Apply a state update to a Handle frame.
    #[allow(clippy::expect_used)]
    fn apply_state_update(&mut self, handle_frame_id: FrameId, state_update: StateUpdate) {
        if let StateUpdate::Updated { value } = state_update {
            let frame = self
                .frames
                .get_mut(handle_frame_id)
                .expect("Handle frame exists");
            let FrameKind::Handle(ref mut handle) = frame.kind else {
                unreachable!("apply_state_update on non-Handle frame");
            };
            handle.state = value;
        }
    }

    /// Resume the body at the Perform site.
    #[allow(clippy::expect_used)]
    fn resume_continuation(
        &mut self,
        handle_frame_id: FrameId,
        value: Value,
    ) -> Result<Option<Value>, CompleteError> {
        let frame = self
            .frames
            .get_mut(handle_frame_id)
            .expect("Handle frame exists");
        let FrameKind::Handle(ref mut handle) = frame.kind else {
            unreachable!("resume_continuation on non-Handle frame");
        };
        let HandleStatus::Suspended(perform_parent) = handle.status else {
            unreachable!("resume_continuation: Handle must be Suspended");
        };
        handle.status = HandleStatus::Free;

        match self.try_deliver(perform_parent, value)? {
            TryDeliverResult::Delivered(result) => Ok(result),
            TryDeliverResult::Blocked(value) => {
                self.stashed_items.push_back(StashedItem::Delivery {
                    parent_ref: perform_parent,
                    value,
                });
                Ok(None)
            }
            TryDeliverResult::FrameGone => Ok(None),
        }
    }

    /// Restart the body. Tear down, re-advance from the body action.
    #[allow(clippy::expect_used)]
    fn restart_body(
        &mut self,
        handle_frame_id: FrameId,
        value: Value,
    ) -> Result<Option<Value>, CompleteError> {
        self.teardown_body(handle_frame_id);
        let frame = self
            .frames
            .get_mut(handle_frame_id)
            .expect("Handle frame exists");
        let FrameKind::Handle(ref mut handle) = frame.kind else {
            unreachable!("restart_body on non-Handle frame");
        };
        let body_action_id = handle.body;
        handle.status = HandleStatus::Free;
        self.advance(
            body_action_id,
            value,
            Some(ParentRef::Handle {
                frame_id: handle_frame_id,
                side: HandleSide::Body,
            }),
        )?;
        Ok(None)
    }

    /// Remove all frames that are descendants of the given Handle's body.
    fn teardown_body(&mut self, handle_frame_id: FrameId) {
        // Collect frame IDs to remove (can't mutate arena while iterating).
        let to_remove: Vec<FrameId> = self
            .frames
            .iter()
            .filter_map(|(frame_id, _)| {
                if self.is_descendant_of_body(frame_id, handle_frame_id) {
                    Some(frame_id)
                } else {
                    None
                }
            })
            .collect();

        for frame_id in &to_remove {
            self.frames.remove(*frame_id);
        }

        // Remove task_to_frame entries whose Invoke frame was torn down.
        self.task_to_frame
            .retain(|_, frame_id| !to_remove.contains(frame_id));
    }

    /// Is `frame_id` a descendant of the given Handle's body side?
    fn is_descendant_of_body(&self, frame_id: FrameId, handle_frame_id: FrameId) -> bool {
        let mut current_id = frame_id;
        loop {
            let Some(frame) = self.frames.get(current_id) else {
                return false;
            };
            let Some(parent_ref) = frame.parent else {
                return false;
            };
            if parent_ref.frame_id() == handle_frame_id {
                // This frame's parent is the Handle. Check if it's on the body side.
                return matches!(
                    parent_ref,
                    ParentRef::Handle {
                        side: HandleSide::Body,
                        ..
                    }
                );
            }
            current_id = parent_ref.frame_id();
        }
    }

    // -- Sweep stash --

    /// Repeatedly sweep the stash until no progress is made.
    fn sweep_stash(&mut self) -> Result<Option<Value>, CompleteError> {
        loop {
            match self.sweep_stash_once()? {
                SweepResult::WorkflowDone(value) => return Ok(Some(value)),
                SweepResult::MadeProgress => {}
                SweepResult::NoProgress => return Ok(None),
            }
        }
    }

    /// Single pass over items that existed at the start.
    #[allow(clippy::expect_used)]
    fn sweep_stash_once(&mut self) -> Result<SweepResult, CompleteError> {
        let n = self.stashed_items.len();
        for _ in 0..n {
            let item = self.stashed_items.pop_front().expect("stash has n items");
            match item {
                StashedItem::Delivery { parent_ref, value } => {
                    match self.try_deliver(parent_ref, value)? {
                        TryDeliverResult::Delivered(Some(value)) => {
                            return Ok(SweepResult::WorkflowDone(value));
                        }
                        TryDeliverResult::Delivered(None) => {
                            return Ok(SweepResult::MadeProgress);
                        }
                        TryDeliverResult::Blocked(value) => {
                            self.stashed_items
                                .push_back(StashedItem::Delivery { parent_ref, value });
                        }
                        TryDeliverResult::FrameGone => {
                            // Frame torn down. Item silently dropped.
                        }
                    }
                }
                StashedItem::Effect {
                    starting_parent,
                    effect_id,
                    payload,
                } => match self.bubble_effect(starting_parent, effect_id, payload)? {
                    StashOutcome::Consumed => {
                        return Ok(SweepResult::MadeProgress);
                    }
                    StashOutcome::Blocked(payload) => {
                        self.stashed_items.push_back(StashedItem::Effect {
                            starting_parent,
                            effect_id,
                            payload,
                        });
                    }
                },
            }
        }
        Ok(SweepResult::NoProgress)
    }

    /// Deliver a value to the parent that was waiting for it.
    ///
    /// - **No parent:** workflow done — return the terminal value.
    /// - **Chain:** trampoline — advance the `rest` action with the value.
    /// - **All/ForEach:** store in results slot; if all slots filled,
    ///   collect into array and deliver to parent.
    #[allow(clippy::expect_used, clippy::unwrap_used)]
    fn deliver(
        &mut self,
        parent: Option<ParentRef>,
        value: Value,
    ) -> Result<Option<Value>, CompleteError> {
        let Some(parent_ref) = parent else {
            return Ok(Some(value));
        };

        match parent_ref {
            ParentRef::Chain { frame_id } => {
                let frame = self.frames.remove(frame_id).expect("parent frame exists");
                let FrameKind::Chain { rest } = frame.kind else {
                    unreachable!(
                        "Chain ParentRef points to non-Chain frame: {:?}",
                        frame.kind
                    )
                };
                self.advance(rest, value, frame.parent)?;
                Ok(None)
            }

            ParentRef::Handle { frame_id, side } => {
                match side {
                    HandleSide::Body => {
                        // Body completed normally (no Perform fired).
                        // Deliver to the Handle's parent.
                        let frame = self.frames.remove(frame_id).expect("parent frame exists");
                        self.deliver(frame.parent, value)
                    }
                    HandleSide::Handler => {
                        // Handler completed. Process the output.
                        self.handle_handler_completion(frame_id, value)
                    }
                }
            }

            ParentRef::All {
                frame_id,
                child_index,
            }
            | ParentRef::ForEach {
                frame_id,
                child_index,
            } => {
                let frame = self.frames.get_mut(frame_id).expect("parent frame exists");
                let results = match &mut frame.kind {
                    FrameKind::All { results } | FrameKind::ForEach { results } => results,
                    other => {
                        unreachable!("All/ForEach ParentRef points to wrong frame: {:?}", other)
                    }
                };
                results[child_index] = Some(value);
                if results.iter().all(Option::is_some) {
                    let collected: Vec<Value> =
                        results.iter_mut().map(|r| r.take().unwrap()).collect();
                    let parent = frame.parent;
                    self.frames.remove(frame_id);
                    self.deliver(parent, Value::Array(collected))
                } else {
                    Ok(None)
                }
            }
        }
    }

    /// Expand an `ActionId` into frames. Creates frames for structural
    /// combinators and Invoke leaves. Invoke frames hold the parent pointer
    /// and handler ID; they're removed when the task completes.
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
    #[allow(
        clippy::too_many_lines,
        clippy::missing_panics_doc,
        clippy::expect_used
    )]
    pub fn advance(
        &mut self,
        action_id: ActionId,
        value: Value,
        parent: Option<ParentRef>,
    ) -> Result<(), AdvanceError> {
        match self.flat_config.action(action_id) {
            FlatAction::Invoke { handler } => {
                let task_id = self.next_task_id();
                let frame_id = self.insert_frame(Frame {
                    parent,
                    kind: FrameKind::Invoke { handler },
                });
                self.task_to_frame.insert(task_id, frame_id);
                self.pending_dispatches.push(Dispatch {
                    task_id,
                    handler_id: handler,
                    value,
                });
            }

            FlatAction::Chain { rest } => {
                let first = self.flat_config.chain_first(action_id);
                let frame_id = self.insert_frame(Frame {
                    parent,
                    kind: FrameKind::Chain { rest },
                });
                self.advance(first, value, Some(ParentRef::Chain { frame_id }))?;
            }

            FlatAction::All { count } => {
                if count.0 == 0 {
                    // No children — vacuously complete with empty array.
                    self.deliver(parent, Value::Array(vec![]))
                        .expect("vacuous empty-parallel completion should not fail");
                    return Ok(());
                }
                // Collect to a Vec to release the immutable borrow on
                // flat_config before the mutable self.advance() calls.
                #[allow(clippy::needless_collect)]
                let children: Vec<ActionId> =
                    self.flat_config.parallel_children(action_id).collect();
                let frame_id = self.insert_frame(Frame {
                    parent,
                    kind: FrameKind::All {
                        results: vec![None; count.0 as usize],
                    },
                });
                for (i, child) in children.into_iter().enumerate() {
                    self.advance(
                        child,
                        value.clone(),
                        Some(ParentRef::All {
                            frame_id,
                            child_index: i,
                        }),
                    )?;
                }
            }

            FlatAction::ForEach { body } => {
                let elements = match value {
                    Value::Array(elements) => elements,
                    other => {
                        return Err(AdvanceError::ForEachExpectedArray { value: other });
                    }
                };
                if elements.is_empty() {
                    // No elements — vacuously complete with empty array.
                    self.deliver(parent, Value::Array(vec![]))
                        .expect("vacuous empty-foreach completion should not fail");
                    return Ok(());
                }
                let frame_id = self.insert_frame(Frame {
                    parent,
                    kind: FrameKind::ForEach {
                        results: vec![None; elements.len()],
                    },
                });
                for (i, element) in elements.into_iter().enumerate() {
                    self.advance(
                        body,
                        element,
                        Some(ParentRef::ForEach {
                            frame_id,
                            child_index: i,
                        }),
                    )?;
                }
            }

            FlatAction::Branch { .. } => {
                let kind_str =
                    value["kind"]
                        .as_str()
                        .ok_or_else(|| AdvanceError::BranchMissingKind {
                            value: value.clone(),
                        })?;
                let (_, case_action_id) = self
                    .flat_config
                    .branch_cases(action_id)
                    .find(|(key, _)| key.lookup() == kind_str)
                    .ok_or_else(|| AdvanceError::BranchNoMatch {
                        kind: kind_str.to_owned(),
                    })?;
                self.advance(case_action_id, value, parent)?;
            }

            FlatAction::Step { target } => {
                self.advance(target, value, parent)?;
            }

            FlatAction::Handle { effect_id } => {
                let body = self.flat_config.handle_body(action_id);
                let handler = self.flat_config.handle_handler(action_id);
                let frame_id = self.insert_frame(Frame {
                    parent,
                    kind: FrameKind::Handle(HandleFrame {
                        effect_id,
                        body,
                        handler,
                        state: value.clone(),
                        status: HandleStatus::Free,
                    }),
                });
                self.advance(
                    body,
                    value,
                    Some(ParentRef::Handle {
                        frame_id,
                        side: HandleSide::Body,
                    }),
                )?;
            }

            FlatAction::Perform { effect_id } => {
                let parent = parent.ok_or(AdvanceError::UnhandledEffect { effect_id })?;
                match self.bubble_effect(parent, effect_id, value)? {
                    StashOutcome::Consumed => {}
                    StashOutcome::Blocked(payload) => {
                        self.stashed_items.push_back(StashedItem::Effect {
                            starting_parent: parent,
                            effect_id,
                            payload,
                        });
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[allow(clippy::doc_markdown)]
mod tests {
    use super::*;
    use barnum_ast::flat::flatten;
    use barnum_ast::*;
    use intern::string_key::Intern;
    use serde_json::json;
    use std::collections::HashMap;

    // -- Helpers --

    fn ts_handler(module: &str, func: &str) -> HandlerKind {
        HandlerKind::TypeScript(TypeScriptHandler {
            module: ModulePath::from(module.intern()),
            func: FuncName::from(func.intern()),
        })
    }

    fn invoke(module: &str, func: &str) -> Action {
        Action::Invoke(InvokeAction {
            handler: ts_handler(module, func),
        })
    }

    fn chain(first: Action, rest: Action) -> Action {
        Action::Chain(ChainAction {
            first: Box::new(first),
            rest: Box::new(rest),
        })
    }

    fn parallel(actions: Vec<Action>) -> Action {
        Action::All(AllAction { actions })
    }

    fn for_each(action: Action) -> Action {
        Action::ForEach(ForEachAction {
            action: Box::new(action),
        })
    }

    fn branch(cases: Vec<(&str, Action)>) -> Action {
        Action::Branch(BranchAction {
            cases: cases
                .into_iter()
                .map(|(k, v)| (KindDiscriminator::from(k.intern()), v))
                .collect(),
        })
    }

    fn step_named(name: &str) -> Action {
        Action::Step(StepAction {
            step: StepRef::Named {
                name: StepName::from(name.intern()),
            },
        })
    }

    #[allow(clippy::unwrap_used)]
    fn engine_from(workflow: Action) -> WorkflowState {
        let config = Config {
            workflow,
            steps: HashMap::new(),
        };
        WorkflowState::new(flatten(config).unwrap())
    }

    #[allow(clippy::unwrap_used)]
    fn engine_from_config(config: Config) -> WorkflowState {
        WorkflowState::new(flatten(config).unwrap())
    }

    // -- Advance tests --

    /// Single invoke: advance -> 1 dispatch.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn single_invoke() {
        let mut engine = engine_from(invoke("./handler.ts", "run"));
        let root = engine.workflow_root();
        engine.advance(root, json!({"x": 1}), None).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].value, json!({"x": 1}));
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./handler.ts", "run"),
        );
    }

    /// Chain(A, B): only A is dispatched on advance.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn chain_dispatches_first_only() {
        let mut engine = engine_from(chain(invoke("./a.ts", "a"), invoke("./b.ts", "b")));
        let root = engine.workflow_root();
        engine.advance(root, json!(null), None).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./a.ts", "a"),
        );
    }

    /// All(A, B, C): all 3 dispatched on advance, all receive the same
    /// input.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn parallel_dispatches_all() {
        let mut engine = engine_from(parallel(vec![
            invoke("./a.ts", "a"),
            invoke("./b.ts", "b"),
            invoke("./c.ts", "c"),
        ]));
        let root = engine.workflow_root();
        engine.advance(root, json!({"shared": true}), None).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 3);
        for d in &dispatches {
            assert_eq!(d.value, json!({"shared": true}));
        }
    }

    /// `ForEach` over 3-element array: 3 dispatches, one per element.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn foreach_dispatches_per_element() {
        let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
        let root = engine.workflow_root();
        engine.advance(root, json!([10, 20, 30]), None).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 3);
        assert_eq!(dispatches[0].value, json!(10));
        assert_eq!(dispatches[1].value, json!(20));
        assert_eq!(dispatches[2].value, json!(30));
    }

    /// Branch: only the matching case is dispatched.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn branch_dispatches_matching_case() {
        let mut engine = engine_from(branch(vec![
            ("Ok", invoke("./ok.ts", "handle")),
            ("Err", invoke("./err.ts", "handle")),
        ]));
        let root = engine.workflow_root();
        engine
            .advance(root, json!({"kind": "Ok", "value": 42}), None)
            .unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./ok.ts", "handle"),
        );
    }

    /// Step(Named): follows the step reference to the target action.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn step_follows_named() {
        let config = Config {
            workflow: step_named("setup"),
            steps: HashMap::from([(
                StepName::from("setup".intern()),
                invoke("./setup.ts", "run"),
            )]),
        };
        let mut engine = engine_from_config(config);
        let root = engine.workflow_root();
        engine.advance(root, json!(null), None).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./setup.ts", "run"),
        );
    }

    /// Nested: Chain inside All. All(Chain(A, B), C) -> dispatches A
    /// and C.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn nested_chain_in_parallel() {
        let mut engine = engine_from(parallel(vec![
            chain(invoke("./a.ts", "a"), invoke("./b.ts", "b")),
            invoke("./c.ts", "c"),
        ]));
        let root = engine.workflow_root();
        engine.advance(root, json!(null), None).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 2);
        let handlers: Vec<_> = dispatches
            .iter()
            .map(|d| engine.handler(d.handler_id).clone())
            .collect();
        assert!(handlers.contains(&ts_handler("./a.ts", "a")));
        assert!(handlers.contains(&ts_handler("./c.ts", "c")));
        // B is not dispatched yet (behind Chain).
        assert!(!handlers.contains(&ts_handler("./b.ts", "b")));
    }

    /// Deep chain: Chain(A, Chain(B, C)) -> only A dispatched.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn deep_chain_dispatches_first_only() {
        let mut engine = engine_from(chain(
            invoke("./a.ts", "a"),
            chain(invoke("./b.ts", "b"), invoke("./c.ts", "c")),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!(null), None).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./a.ts", "a"),
        );
    }

    /// `ForEach` with empty array: no dispatches, immediate completion.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn foreach_empty_array() {
        let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
        let root = engine.workflow_root();
        engine.advance(root, json!([]), None).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 0);
    }

    /// All with empty children: no dispatches, immediate completion.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn parallel_empty() {
        let mut engine = engine_from(parallel(vec![]));
        let root = engine.workflow_root();
        engine.advance(root, json!(null), None).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 0);
    }

    // -- Completion tests --

    /// Chain(A, B): complete A -> dispatches B. Complete B -> workflow done.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn chain_trampolines_on_completion() {
        let mut engine = engine_from(chain(invoke("./a.ts", "a"), invoke("./b.ts", "b")));
        let root = engine.workflow_root();
        engine.advance(root, json!(null), None).unwrap();

        let d1 = engine.take_pending_dispatches();
        assert_eq!(d1.len(), 1);

        let result = engine.complete(d1[0].task_id, json!("a_result")).unwrap();
        assert_eq!(result, None);

        let d2 = engine.take_pending_dispatches();
        assert_eq!(d2.len(), 1);
        assert_eq!(d2[0].value, json!("a_result"));

        let result = engine.complete(d2[0].task_id, json!("b_result")).unwrap();
        assert_eq!(result, Some(json!("b_result")));
    }

    /// Deep chain: Chain(A, Chain(B, C)) -> A -> B -> C -> done.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn nested_chain_completes() {
        let mut engine = engine_from(chain(
            invoke("./a.ts", "a"),
            chain(invoke("./b.ts", "b"), invoke("./c.ts", "c")),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // A
        let d = engine.take_pending_dispatches();
        assert_eq!(engine.complete(d[0].task_id, json!("a_out")).unwrap(), None);
        // B
        let d = engine.take_pending_dispatches();
        assert_eq!(d[0].value, json!("a_out"));
        assert_eq!(engine.complete(d[0].task_id, json!("b_out")).unwrap(), None);
        // C
        let d = engine.take_pending_dispatches();
        assert_eq!(d[0].value, json!("b_out"));
        assert_eq!(
            engine.complete(d[0].task_id, json!("c_out")).unwrap(),
            Some(json!("c_out")),
        );
    }

    /// `All(A, B)`: complete both -> workflow done with collected results.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn parallel_collects_results() {
        let mut engine = engine_from(parallel(vec![invoke("./a.ts", "a"), invoke("./b.ts", "b")]));
        let root = engine.workflow_root();
        engine.advance(root, json!(null), None).unwrap();

        let d = engine.take_pending_dispatches();
        assert_eq!(d.len(), 2);

        // Complete in reverse order to verify index-based collection.
        assert_eq!(
            engine.complete(d[1].task_id, json!("b_result")).unwrap(),
            None,
        );
        assert_eq!(
            engine.complete(d[0].task_id, json!("a_result")).unwrap(),
            Some(json!(["a_result", "b_result"])),
        );
    }

    /// `ForEach` over array: complete both -> collected results.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn foreach_collects_results() {
        let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
        let root = engine.workflow_root();
        engine.advance(root, json!([10, 20]), None).unwrap();

        let d = engine.take_pending_dispatches();
        assert_eq!(d.len(), 2);

        assert_eq!(engine.complete(d[0].task_id, json!("r10")).unwrap(), None);
        assert_eq!(
            engine.complete(d[1].task_id, json!("r20")).unwrap(),
            Some(json!(["r10", "r20"])),
        );
    }

    // -- Arena / generational safety --

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

    // -- Handle / Perform helpers --

    fn handle(effect_id: u16, handler: Action, body: Action) -> Action {
        Action::Handle(HandleAction {
            effect_id: EffectId(effect_id),
            body: Box::new(body),
            handler: Box::new(handler),
        })
    }

    fn perform(effect_id: u16) -> Action {
        Action::Perform(PerformAction {
            effect_id: EffectId(effect_id),
        })
    }

    fn invoke_builtin(builtin: BuiltinKind) -> Action {
        Action::Invoke(InvokeAction {
            handler: HandlerKind::Builtin(BuiltinHandler { builtin }),
        })
    }

    fn constant_handler(value: Value) -> Action {
        invoke_builtin(BuiltinKind::Constant { value })
    }

    #[allow(clippy::needless_pass_by_value)]
    fn always_resume_handler(value: Value) -> Action {
        constant_handler(json!({
            "kind": "Resume",
            "value": value,
        }))
    }

    fn tag_builtin(kind: &str) -> Action {
        invoke_builtin(BuiltinKind::Tag { value: json!(kind) })
    }

    fn extract_field(field: &str) -> Action {
        invoke_builtin(BuiltinKind::ExtractField {
            value: json!(field),
        })
    }

    fn extract_index(index: u64) -> Action {
        invoke_builtin(BuiltinKind::ExtractIndex {
            value: json!(index),
        })
    }

    fn identity_action() -> Action {
        invoke_builtin(BuiltinKind::Identity)
    }

    /// Handler for restart+Branch: extract payload (index 0), tag RestartBody.
    fn restart_body_handler() -> Action {
        chain(extract_index(0), tag_builtin("RestartBody"))
    }

    /// Chain(Tag("Break"), Perform(effect_id)) — triggers restart with Break routing.
    fn break_perform(effect_id: u16) -> Action {
        chain(tag_builtin("Break"), perform(effect_id))
    }

    /// Build restart+Branch compiled form:
    /// Chain(Tag("Continue"), Handle(effectId, Branch({
    ///   Continue: Chain(ExtractField("value"), continueArm),
    ///   Break: Chain(ExtractField("value"), breakArm),
    /// }), RestartBodyHandler))
    fn restart_branch(effect_id: u16, continue_arm: Action, break_arm: Action) -> Action {
        chain(
            tag_builtin("Continue"),
            handle(
                effect_id,
                restart_body_handler(),
                branch(vec![
                    ("Continue", chain(extract_field("value"), continue_arm)),
                    ("Break", chain(extract_field("value"), break_arm)),
                ]),
            ),
        )
    }

    fn echo_resume_handler() -> Action {
        chain(
            invoke_builtin(BuiltinKind::ExtractIndex { value: json!(0) }),
            invoke_builtin(BuiltinKind::Tag {
                value: json!("Resume"),
            }),
        )
    }

    fn garbage_output_handler() -> Action {
        constant_handler(json!({ "kind": "Unknown" }))
    }

    fn missing_fields_handler() -> Action {
        constant_handler(json!({ "kind": "Resume" }))
    }

    /// Process all pending builtin dispatches. Returns TypeScript dispatches
    /// for manual completion and workflow result (if the workflow terminated).
    #[allow(clippy::unwrap_used, clippy::type_complexity)]
    fn drive_builtins(
        engine: &mut WorkflowState,
    ) -> Result<(Option<Value>, Vec<Dispatch>), CompleteError> {
        let mut ts_dispatches: Vec<Dispatch> = Vec::new();
        loop {
            let dispatches = engine.take_pending_dispatches();
            if dispatches.is_empty() {
                break;
            }
            let mut had_builtin = false;
            for dispatch in dispatches {
                match engine.handler(dispatch.handler_id).clone() {
                    HandlerKind::Builtin(builtin_handler) => {
                        let result = barnum_builtins::execute_builtin(
                            &builtin_handler.builtin,
                            &dispatch.value,
                        )
                        .unwrap();
                        if let Some(value) = engine.complete(dispatch.task_id, result)? {
                            return Ok((Some(value), ts_dispatches));
                        }
                        had_builtin = true;
                    }
                    HandlerKind::TypeScript(_) => {
                        ts_dispatches.push(dispatch);
                    }
                }
            }
            if !had_builtin {
                break;
            }
        }
        Ok((None, ts_dispatches))
    }

    /// Complete a task and then drive all resulting builtins.
    #[allow(clippy::unwrap_used)]
    fn complete_and_drive(
        engine: &mut WorkflowState,
        task_id: TaskId,
        value: Value,
    ) -> Result<(Option<Value>, Vec<Dispatch>), CompleteError> {
        let result = engine.complete(task_id, value)?;
        if result.is_some() {
            let ts = engine.take_pending_dispatches();
            return Ok((result, ts));
        }
        drive_builtins(engine)
    }

    // -- Handle / Perform tests --

    // Test 1: Bare Perform with no enclosing Handle → UnhandledEffect.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn perform_without_handle_errors() {
        let mut engine = engine_from(perform(1));
        let root = engine.workflow_root();
        let err = engine.advance(root, json!(null), None).unwrap_err();
        assert!(
            matches!(err, AdvanceError::UnhandledEffect { effect_id } if effect_id == EffectId(1)),
            "expected UnhandledEffect, got: {err:?}",
        );
    }

    // Test 2: Handle(e, always_resume(42), Chain(Perform(e), Invoke(echo))).
    /// Perform fires, handler resumes with 42, Chain trampolines to echo with 42.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn handle_resume_chains_to_next() {
        let mut engine = engine_from(handle(
            1,
            always_resume_handler(json!(42)),
            chain(perform(1), invoke("./echo.ts", "echo")),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Perform fires synchronously during advance. The handler DAG
        // (Constant) produces a dispatch. Drive builtins to process it.
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        // Chain trampolines to echo with the resumed value.
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!(42));

        // Complete echo.
        let result = engine
            .complete(ts[0].task_id, json!("echo_result"))
            .unwrap();
        assert_eq!(result, Some(json!("echo_result")));
    }

    /// Test 3: restart+Branch where Break+Perform fires before Invoke(unreachable).
    /// Handler restarts body, Branch takes Break arm, Invoke never runs.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn restart_branch_break_skips_rest_of_chain() {
        let mut engine = engine_from(restart_branch(
            1,
            chain(break_perform(1), invoke("./unreachable.ts", "nope")),
            identity_action(),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, Some(json!("input")));
        // No TypeScript dispatches — unreachable invoke never ran.
        assert!(ts.is_empty());
    }

    /// Test 5: Perform(outer) skips inner Handle, caught by outer.
    /// Handle(outer, h_outer, Handle(inner, h_inner, Perform(outer))).
    #[test]
    #[allow(clippy::unwrap_used)]
    fn perform_skips_non_matching_handle() {
        let mut engine = engine_from(handle(
            1,
            always_resume_handler(json!("outer_handled")),
            handle(
                2,
                always_resume_handler(json!("inner_handled")),
                perform(1), // effect_id=1, skips inner (effect_id=2)
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        // Outer handler resumed, inner Handle's body gets the value.
        // But Perform(1) is the entire inner body, so delivery goes to
        // Handle(inner)'s parent = Handle(outer)'s body.
        // Inner Handle exits with "outer_handled", then outer Handle exits.
        assert_eq!(result, Some(json!("outer_handled")));
        assert!(ts.is_empty());
    }

    /// Test 6: Perform is first half of Chain. After Resume(V), Chain rest receives V.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn perform_in_chain_first_resumes_to_rest() {
        let mut engine = engine_from(handle(
            1,
            always_resume_handler(json!("resumed_value")),
            chain(perform(1), invoke("./next.ts", "next")),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("resumed_value"));

        let result = engine.complete(ts[0].task_id, json!("final")).unwrap();
        assert_eq!(result, Some(json!("final")));
    }

    /// Test 7: One branch of All Performs, the other completes normally.
    /// After Resume, All joins both.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn handle_all_one_performs_one_normal() {
        // Handle(e, echo_resume, All(Chain(Invoke(A), Perform(e)), Invoke(B)))
        let mut engine = engine_from(handle(
            1,
            echo_resume_handler(),
            parallel(vec![
                chain(invoke("./a.ts", "a"), perform(1)),
                invoke("./b.ts", "b"),
            ]),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 2); // A and B dispatched

        // Complete A -> Chain trampolines to Perform -> handler dispatched.
        let (result, ts2) = complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, None);
        // echo_resume echoes "a_out" back. Resume delivers to All slot 0.
        assert!(ts2.is_empty());

        // Complete B -> All slot 1.
        let result = engine.complete(ts[1].task_id, json!("b_out")).unwrap();
        // All joins with [resumed_value, "b_out"].
        assert_eq!(result, Some(json!(["a_out", "b_out"])));
    }

    /// Test 8: After restart+Branch Break, body frames removed from arena, task_to_parent
    /// entries removed. Chain(Invoke(A), break_perform) in Continue arm.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn restart_branch_break_cleans_up_frames_and_tasks() {
        let mut engine = engine_from(restart_branch(
            1,
            chain(invoke("./a.ts", "a"), break_perform(1)),
            identity_action(),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // A dispatched.
        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 1);

        // Complete A → break_perform → handler restarts → Branch(Break) → identity → exits.
        let (result, _) = complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, Some(json!("a_out")));

        // Verify frames are empty.
        assert_eq!(engine.frames.len(), 0);
        assert!(engine.task_to_frame.is_empty());
    }

    /// Test 9: Body Performs twice in a Chain. First resumed, then second fires.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn two_performs_in_chain() {
        // Handle(e, echo_resume, Chain(Perform(e), Chain(Invoke(mid), Perform(e))))
        let mut engine = engine_from(handle(
            1,
            echo_resume_handler(),
            chain(perform(1), chain(invoke("./mid.ts", "mid"), perform(1))),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // First Perform fires during advance. Drive builtins for echo_resume handler.
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        // Handler echoed "input" back. Chain trampolines to Chain(mid, Perform).
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("input"));

        // Complete mid -> inner Chain trampolines to second Perform -> handler ->
        // Resume with "mid_out" -> deliver to Handle{Body} -> body done -> Handle exits.
        let (result, ts2) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("mid_out")).unwrap();
        assert_eq!(result, Some(json!("mid_out")));
        assert!(ts2.is_empty());
    }

    /// Test 10: RestartBody multiple times via Continue, then exit via Break.
    /// Uses restart+Branch pattern. Body step returns pre-tagged Continue or Break values.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn restart_branch_multiple_then_break() {
        // Continue arm: Invoke(step) → Perform(1).
        // Step returns { kind: "Continue", value: ... } or { kind: "Break", value: ... }.
        // The Perform payload is the pre-tagged value; handler extracts and restarts.
        // Branch dispatches on the tag.
        let mut engine = engine_from(restart_branch(
            1,
            chain(invoke("./body.ts", "step"), perform(1)),
            identity_action(),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("init"), None).unwrap();

        // drive_builtins processes the Tag("Continue"), Branch dispatch builtins,
        // and returns the TypeScript step Invoke dispatch.
        let (_, mut body_dispatches) = drive_builtins(&mut engine).unwrap();
        assert_eq!(body_dispatches.len(), 1);

        let frame_count_before = engine.frames.len();

        for _ in 0..3 {
            // Complete step with Continue-tagged value → Perform → handler restarts →
            // Branch(Continue) → re-enter body. complete_and_drive drives all builtins
            // and returns the new TypeScript step dispatch.
            let (result, new_dispatches) = complete_and_drive(
                &mut engine,
                body_dispatches[0].task_id,
                json!({"kind": "Continue", "value": "restarted"}),
            )
            .unwrap();
            assert_eq!(result, None);

            body_dispatches = new_dispatches;
            assert_eq!(body_dispatches.len(), 1);
            assert_eq!(body_dispatches[0].value, json!("restarted"));
        }

        // Frames shouldn't grow across restarts.
        assert!(
            engine.frames.len() <= frame_count_before,
            "arena grew: {} -> {}",
            frame_count_before,
            engine.frames.len(),
        );

        // Final iteration: Break instead of Continue.
        let (result, _) = complete_and_drive(
            &mut engine,
            body_dispatches[0].task_id,
            json!({"kind": "Break", "value": "gave_up"}),
        )
        .unwrap();
        assert_eq!(result, Some(json!("gave_up")));
        assert_eq!(engine.frames.len(), 0);
    }

    /// Test 14: Body runs without Performing. Handle exits normally.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn handle_body_no_perform_exits_normally() {
        let mut engine = engine_from(handle(
            1,
            always_resume_handler(json!("unused")),
            invoke("./body.ts", "run"),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 1); // body invoke dispatched

        let result = engine
            .complete(ts[0].task_id, json!("body_result"))
            .unwrap();
        // Body done, no Perform, Handle exits with body result.
        assert_eq!(result, Some(json!("body_result")));
    }

    /// Test 15: Stash scenario — completion during suspension.
    /// Handle(e, echo_resume, All(Chain(Invoke(A), Perform(e)), Invoke(B)))
    /// Complete A -> Perform -> handler. Complete B -> stashed. Handler resumes -> sweep -> B delivered -> All joins.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn stash_delivery_during_suspension() {
        let mut engine = engine_from(handle(
            1,
            invoke("./handler.ts", "handler"),
            parallel(vec![
                chain(invoke("./a.ts", "a"), perform(1)),
                invoke("./b.ts", "b"),
            ]),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 2); // A and B

        // Complete A -> Chain trampolines to Perform -> handler dispatched.
        let (result, handler_ts) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, None);
        assert_eq!(handler_ts.len(), 1); // handler TS dispatch

        // Complete B while handler is running -> stashed.
        let result = engine.complete(ts[1].task_id, json!("b_out")).unwrap();
        assert_eq!(result, None);
        assert_eq!(engine.stashed_items.len(), 1);

        // Complete handler with Resume.
        let (result, _) = complete_and_drive(
            &mut engine,
            handler_ts[0].task_id,
            json!({"kind": "Resume", "value": "a_resumed"}),
        )
        .unwrap();
        // Resume delivers to All slot 0. sweep_stash delivers B to All slot 1. All joins.
        assert_eq!(result, Some(json!(["a_resumed", "b_out"])));
        assert!(engine.stashed_items.is_empty());
    }

    /// Test 16: restart+Branch with concurrent tasks. A completes → break_perform → restart →
    /// Branch(Break) → exits. B's in-flight task is torn down with the body.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn teardown_cleans_up_concurrent_tasks() {
        let mut engine = engine_from(restart_branch(
            1,
            parallel(vec![
                chain(invoke("./a.ts", "a"), break_perform(1)),
                invoke("./b.ts", "b"),
            ]),
            identity_action(),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 2);

        // Complete A → break_perform → handler (builtin) → RestartBody → Branch(Break) → exits.
        // B's task_to_frame entry is cleaned up during body teardown.
        let (result, _) = complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, Some(json!("a_out")));
        assert_eq!(engine.frames.len(), 0);
        assert!(engine.task_to_frame.is_empty());
    }

    /// Test 22: All(Perform(e), Perform(e)) — concurrent Performs.
    /// First dispatches handler, second stashed. After resume, sweep retries.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn concurrent_performs_serialized() {
        let mut engine = engine_from(handle(
            1,
            invoke("./handler.ts", "handler"),
            parallel(vec![perform(1), perform(1)]),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // During advance: All advances both Performs.
        // First Perform: bubble_effect -> dispatch handler (Handle becomes Suspended).
        // Second Perform: find_blocking_ancestor -> Handle busy -> stashed as Effect.
        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 1); // handler dispatch for first Perform
        assert_eq!(engine.stashed_items.len(), 1); // second Perform stashed

        // Complete first handler with Resume.
        let (result, handler2_ts) = complete_and_drive(
            &mut engine,
            ts[0].task_id,
            json!({"kind": "Resume", "value": "first_resumed"}),
        )
        .unwrap();
        // Resume delivers to All slot 0. sweep_stash retries second effect ->
        // Handle free -> dispatches second handler.
        assert_eq!(result, None);
        assert_eq!(handler2_ts.len(), 1);
        assert!(engine.stashed_items.is_empty());

        // Complete second handler.
        let (result, _) = complete_and_drive(
            &mut engine,
            handler2_ts[0].task_id,
            json!({"kind": "Resume", "value": "second_resumed"}),
        )
        .unwrap();
        assert_eq!(result, Some(json!(["first_resumed", "second_resumed"])),);
    }

    /// Test 24: Handler returns garbage output → InvalidHandlerOutput error.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn garbage_handler_output_errors() {
        let mut engine = engine_from(handle(1, garbage_output_handler(), perform(1)));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Handler Constant returns { kind: "Unknown" }.
        let err = drive_builtins(&mut engine).unwrap_err();
        assert!(
            matches!(err, CompleteError::InvalidHandlerOutput { .. }),
            "expected InvalidHandlerOutput, got: {err:?}",
        );
    }

    /// Test 25: Handler returns { kind: "Resume" } with missing value → InvalidHandlerOutput.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn missing_fields_handler_output_errors() {
        let mut engine = engine_from(handle(1, missing_fields_handler(), perform(1)));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let err = drive_builtins(&mut engine).unwrap_err();
        assert!(
            matches!(err, CompleteError::InvalidHandlerOutput { .. }),
            "expected InvalidHandlerOutput, got: {err:?}",
        );
    }

    /// Test 27: Effect shadowing — inner Handle intercepts same effect_id.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn effect_shadowing_inner_catches() {
        // Handle(e, outer_handler, Handle(e, inner_handler, Perform(e)))
        let mut engine = engine_from(handle(
            1,
            always_resume_handler(json!("outer")),
            handle(1, always_resume_handler(json!("inner")), perform(1)),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Inner Handle catches the Perform. Handler resumes with "inner".
        // Inner body done -> inner Handle exits -> outer body done -> outer Handle exits.
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, Some(json!("inner")));
        assert!(ts.is_empty());
    }

    /// Test 30: Handle(e, handler, Perform(e)) — Perform is direct child of Handle body.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn perform_direct_child_of_handle_body() {
        let mut engine = engine_from(handle(
            1,
            always_resume_handler(json!("resumed")),
            perform(1),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Perform's parent is Handle{Body}. bubble_effect checks is_blocked_by_handle.
        // Handle is Free -> not blocked -> handler dispatched.
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        // Handler resumes with "resumed". Deliver to Handle{Body} -> body done -> exit.
        assert_eq!(result, Some(json!("resumed")));
        assert!(ts.is_empty());
    }

    /// Test 12: resume_with_state_handler — handler extracts state, resumes with it.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn resume_with_state() {
        // Handler DAG: ExtractIndex(1) -> Tag("Resume")
        // This means the handler echoes the Handle's state (index 1) back as the resume value.
        let resume_with_state = chain(
            invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
            invoke_builtin(BuiltinKind::Tag {
                value: json!("Resume"),
            }),
        );
        // Handle's initial state = pipeline value ("input"). Body: Chain(Invoke(step), Perform(e))
        let mut engine = engine_from(handle(
            1,
            resume_with_state,
            chain(invoke("./step.ts", "step"), perform(1)),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 1); // step dispatched

        // Complete step -> Perform fires -> handler gets ["step_out", "input"]
        // Handler extracts state (index 1 = "input") and resumes with it.
        let (result, _) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("step_out")).unwrap();
        // Resume with "input" -> deliver to Chain -> body done -> Handle exits.
        assert_eq!(result, Some(json!("input")));
    }

    /// Test 13: State update persists across handler invocations.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn state_update_persists() {
        // Handler: TS handler we manually complete (so we can control state_update).
        // Body: Chain(Perform(e), Chain(Invoke(mid), Perform(e)))
        let mut engine = engine_from(handle(
            1,
            invoke("./handler.ts", "handler"),
            chain(perform(1), chain(invoke("./mid.ts", "mid"), perform(1))),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // First Perform fires during advance -> handler dispatched.
        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 1);

        // First handler: Resume with state_update.
        let result = engine
            .complete(
                ts[0].task_id,
                json!({
                    "kind": "Resume",
                    "value": "v1",
                    "state_update": { "kind": "Updated", "value": "new_state" }
                }),
            )
            .unwrap();
        assert_eq!(result, None);

        // Chain trampolines to mid.
        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("v1"));

        // Complete mid -> Chain trampolines to second Perform -> handler dispatched.
        let (_, handler_ts) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("mid_out")).unwrap();
        assert_eq!(handler_ts.len(), 1);

        // Second handler receives state = "new_state" (persisted from first handler).
        // We verify by checking the dispatch value: [payload, state].
        assert_eq!(handler_ts[0].value, json!(["mid_out", "new_state"]),);

        // Complete second handler with Resume.
        let (result, _) = complete_and_drive(
            &mut engine,
            handler_ts[0].task_id,
            json!({"kind": "Resume", "value": "v2"}),
        )
        .unwrap();
        // Body done -> Handle exits.
        assert_eq!(result, Some(json!("v2")));
    }

    /// Test 21: Multi-step handler Chain. Handler side is never blocked.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn multi_step_handler_chain() {
        // Handler: Chain(Invoke(step1), Invoke(step2))
        // Body: Perform(e)
        let mut engine = engine_from(handle(
            1,
            chain(invoke("./step1.ts", "s1"), invoke("./step2.ts", "s2")),
            perform(1),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Perform fires -> handler Chain starts -> step1 dispatched.
        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 1);

        // Complete step1 -> Chain trampolines to step2.
        let result = engine.complete(ts[0].task_id, json!("s1_out")).unwrap();
        assert_eq!(result, None);

        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("s1_out"));

        // Complete step2 -> handler done -> handle_handler_completion called.
        // step2 returns the HandlerOutput JSON.
        let result = engine
            .complete(ts[0].task_id, json!({"kind": "Resume", "value": "final"}))
            .unwrap();
        assert_eq!(result, Some(json!("final")));
    }

    // -- Bind-shaped AST tests --
    //
    // These verify the runtime behavior of the Handle/Perform substrate
    // when driven by bind-shaped ASTs. The ASTs are constructed directly
    // in Rust, matching what the TS `bind()` macro produces.

    /// readVar(n): Chain(ExtractIndex(1), Chain(ExtractIndex(n), Tag("Resume")))
    fn read_var(n: u64) -> Action {
        chain(
            invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
            chain(
                invoke_builtin(BuiltinKind::ExtractIndex { value: json!(n) }),
                invoke_builtin(BuiltinKind::Tag {
                    value: json!("Resume"),
                }),
            ),
        )
    }

    /// Bind test 1: Single binding, single read.
    /// Chain(All(Constant(42), Identity), Handle(e0, readVar(0), Chain(Perform(e0), Invoke(echo))))
    /// Input: "input". All → [42, "input"]. Handle state = [42, "input"].
    /// Perform fires, readVar(0) extracts 42, resumes. echo receives 42.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn bind_single_binding_single_read() {
        let e0 = 10;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant { value: json!(42) }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                read_var(0),
                chain(perform(e0), invoke("./echo.ts", "echo")),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!(42)); // echo receives 42

        let result = engine.complete(ts[0].task_id, json!("echo_done")).unwrap();
        assert_eq!(result, Some(json!("echo_done")));
    }

    /// Bind test 2: Single binding, body ignores VarRef.
    /// Chain(All(Constant(42), Identity), Handle(e0, readVar(0), Chain(ExtractIndex(1), Invoke(echo))))
    /// Body extracts pipeline_input (index 1) and passes to echo. No Perform fires.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn bind_single_binding_body_ignores_varref() {
        let e0 = 10;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant { value: json!(42) }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                read_var(0),
                chain(
                    invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
                    invoke("./echo.ts", "echo"),
                ),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("input")); // echo receives pipeline_input

        let result = engine.complete(ts[0].task_id, json!("done")).unwrap();
        assert_eq!(result, Some(json!("done")));
    }

    /// Bind test 3: Two bindings, two reads.
    /// `All(Constant("alice"), Constant(99), Identity)` → `["alice", 99, "input"]`
    /// Nested Handle(e0, readVar(0), Handle(e1, readVar(1), Chain(Perform(e0), Chain(Invoke(mid), Perform(e1)))))
    /// First Perform → "alice", mid gets "alice", second Perform → 99.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn bind_two_bindings_two_reads() {
        let e0 = 10;
        let e1 = 11;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant {
                    value: json!("alice"),
                }),
                invoke_builtin(BuiltinKind::Constant { value: json!(99) }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                read_var(0),
                handle(
                    e1,
                    read_var(1),
                    chain(perform(e0), chain(invoke("./mid.ts", "mid"), perform(e1))),
                ),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // First Perform(e0): bubbles past inner Handle(e1), caught by outer Handle(e0).
        // readVar(0) extracts state[0] = "alice" from [alice, 99, input]. Resumes with "alice".
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        // Chain trampolines to mid with "alice".
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("alice"));

        // Complete mid → second Perform(e1). Inner Handle catches.
        // readVar(1) extracts state[1] = 99. Resumes with 99.
        let (result, ts2) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("mid_out")).unwrap();
        assert_eq!(result, Some(json!(99)));
        assert!(ts2.is_empty());
    }

    /// Bind test 4: Two bindings, reads in reverse order.
    /// Body: Chain(Perform(e1), Perform(e0)).
    /// Perform(e1) caught by inner Handle. Perform(e0) bubbles to outer Handle.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn bind_two_bindings_reverse_order() {
        let e0 = 10;
        let e1 = 11;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant {
                    value: json!("alice"),
                }),
                invoke_builtin(BuiltinKind::Constant { value: json!(99) }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                read_var(0),
                handle(e1, read_var(1), chain(perform(e1), perform(e0))),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Perform(e1) is caught by inner Handle(e1). readVar(1) → state[1] = 99. Resume with 99.
        // Chain trampolines to Perform(e0). Bubbles past inner Handle, caught by outer Handle(e0).
        // readVar(0) → state[0] = "alice". Resume.
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, Some(json!("alice")));
        assert!(ts.is_empty());
    }

    /// Bind test 5: Nested binds.
    /// Outer bind: All("outer", Identity) → Handle(e_outer, readVar(0), ...)
    /// Inner bind: All("inner", Identity) → Handle(e_inner, readVar(0), ...)
    /// Body: Chain(Perform(e_outer), Perform(e_inner))
    /// e_outer bubbles past inner Handle, caught by outer. e_inner caught by inner.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn bind_nested() {
        let e_outer = 10;
        let e_inner = 11;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant {
                    value: json!("outer"),
                }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e_outer,
                read_var(0),
                chain(
                    invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
                    // Inner bind operates on pipeline_input
                    chain(
                        parallel(vec![
                            invoke_builtin(BuiltinKind::Constant {
                                value: json!("inner"),
                            }),
                            invoke_builtin(BuiltinKind::Identity),
                        ]),
                        handle(
                            e_inner,
                            read_var(0),
                            chain(
                                invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
                                chain(perform(e_outer), perform(e_inner)),
                            ),
                        ),
                    ),
                ),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Perform(e_outer) bubbles past inner Handle(e_inner) and Handle(e_outer).
        // Actually wait — Perform(e_outer) is inside inner Handle(e_inner) which is inside
        // outer Handle(e_outer). The bubble order: inner body → inner Handle(e_inner, wrong eid) →
        // outer Handle body → outer Handle(e_outer, matches!). readVar(0) on outer state
        // = ["outer", "input"][0] = "outer". Resume with "outer".
        // Chain to Perform(e_inner). Inner Handle catches. readVar(0) on inner state
        // = ["inner", "input"][0] = "inner". Resume with "inner".
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, Some(json!("inner")));
        assert!(ts.is_empty());
    }

    /// Bind test 6: Bind inside ForEach.
    /// ForEach(Chain(All(Identity, Identity), Handle(e0, readVar(0), Chain(Perform(e0), Invoke(echo)))))
    /// Input [10, 20]. Each iteration binds its own element.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn bind_inside_foreach() {
        let e0 = 10;
        let mut engine = engine_from(for_each(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Identity),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                read_var(0),
                chain(perform(e0), invoke("./echo.ts", "echo")),
            ),
        )));
        let root = engine.workflow_root();
        engine.advance(root, json!([10, 20]), None).unwrap();

        // Both ForEach iterations run. Each binds its own element via Identity.
        // For element 10: All(Identity, Identity) → [10, 10]. Handle state = [10, 10].
        //   Perform → readVar(0) → 10. Resume → echo with 10.
        // For element 20: same with 20.
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        assert_eq!(ts.len(), 2); // Two echo dispatches

        // Complete both echo invocations.
        assert_eq!(engine.complete(ts[0].task_id, json!("r10")).unwrap(), None,);
        assert_eq!(
            engine.complete(ts[1].task_id, json!("r20")).unwrap(),
            Some(json!(["r10", "r20"])),
        );
    }

    /// Bind test 7: Handler receives correct state shape.
    /// Handle(e, TS_handler, Perform(e)). Advance with "input".
    /// State = "input" (Handle initializes from pipeline value).
    /// Handler dispatch value should be ["input", [42, "input"]].
    /// (Perform's input = "input" because body extracts index 1 from the All tuple.)
    #[test]
    #[allow(clippy::unwrap_used)]
    fn bind_handler_receives_correct_state() {
        // Use a TS handler so we can inspect the dispatch value.
        let e0 = 10;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant { value: json!(42) }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                invoke("./handler.ts", "handler"),
                chain(
                    invoke_builtin(BuiltinKind::ExtractIndex { value: json!(1) }),
                    perform(e0),
                ),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        assert_eq!(ts.len(), 1);
        // Handler receives [payload, state] = ["input", [42, "input"]]
        // (state is the All output tuple, which is the pipeline value entering the Handle)
        assert_eq!(ts[0].value, json!(["input", [42, "input"]]),);
    }

    /// Bind test 8: readVar(1) produces correct Resume value.
    /// State = `["a", "b", "c"]`. `readVar(1)` should produce `{ kind: "Resume", value: "b" }`.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn bind_read_var_produces_correct_resume() {
        let e0 = 10;
        let mut engine = engine_from(chain(
            parallel(vec![
                invoke_builtin(BuiltinKind::Constant { value: json!("a") }),
                invoke_builtin(BuiltinKind::Constant { value: json!("b") }),
                invoke_builtin(BuiltinKind::Constant { value: json!("c") }),
                invoke_builtin(BuiltinKind::Identity),
            ]),
            handle(
                e0,
                read_var(1), // Extract state[1] = "b"
                chain(
                    invoke_builtin(BuiltinKind::ExtractIndex { value: json!(3) }),
                    chain(perform(e0), invoke("./echo.ts", "echo")),
                ),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // Perform fires. readVar(1) extracts state[1] = "b". Resumes with "b".
        // Chain trampolines to echo with "b".
        let (result, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(result, None);
        assert_eq!(ts.len(), 1);
        assert_eq!(ts[0].value, json!("b"));

        let result = engine.complete(ts[0].task_id, json!("done")).unwrap();
        assert_eq!(result, Some(json!("done")));
    }

    /// Test 23: Two effects targeting different Handles.
    /// Handle(e1, h1, Handle(e2, h2, All(Perform(e1), Perform(e2))))
    #[test]
    #[allow(clippy::unwrap_used)]
    fn two_effects_different_handles() {
        let mut engine = engine_from(handle(
            1,
            invoke("./h1.ts", "h1"),
            handle(
                2,
                invoke("./h2.ts", "h2"),
                parallel(vec![perform(1), perform(2)]),
            ),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // During advance: All advances both Performs.
        // Perform(e1): bubble_effect walks up from All -> Handle(e2) (wrong effect) ->
        //   Handle(e1) (matches, free) -> dispatches h1. Handle(e1) suspends.
        // Perform(e2): find_blocking_ancestor from All -> Handle(e2) (free, body, not blocked) ->
        //   Handle(e1) (body, suspended) -> Blocked! Stashed as Effect.
        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 1); // h1 dispatch
        assert_eq!(engine.stashed_items.len(), 1);

        // Complete h1 with Resume.
        let (result, h2_ts) = complete_and_drive(
            &mut engine,
            ts[0].task_id,
            json!({"kind": "Resume", "value": "e1_resumed"}),
        )
        .unwrap();
        // Resume delivers to All slot 0. sweep_stash retries Perform(e2) ->
        // Handle(e1) free -> walk to Handle(e2) (matches, free) -> dispatches h2.
        assert_eq!(result, None);
        assert_eq!(h2_ts.len(), 1);

        // Complete h2 with Resume.
        let (result, _) = complete_and_drive(
            &mut engine,
            h2_ts[0].task_id,
            json!({"kind": "Resume", "value": "e2_resumed"}),
        )
        .unwrap();
        assert_eq!(result, Some(json!(["e1_resumed", "e2_resumed"])),);
    }

    // -- Resume handler non-suspension tests --
    //
    // These tests document the desired behavior after the Resume/Restart
    // handler split: resume handlers should NOT suspend the Handle frame,
    // so concurrent siblings should not be blocked.
    //
    // Currently these fail because the engine suspends on ALL Performs.

    /// Resume handler with async handler DAG should not block sibling
    /// completions.
    ///
    /// Setup: Handle(e, ts_handler, All(Chain(Invoke(A), Perform(e)), Invoke(B)))
    /// Branch 0: A completes → Perform(e) → handler dispatched (async).
    /// Branch 1: B completes → should deliver to All slot 1 immediately.
    ///
    /// Currently: B's completion is stashed because Handle is suspended.
    /// After refactor: Handle is never suspended for resume, B delivers.
    #[test]
    #[should_panic(expected = "resume handler should not cause stashing")]
    #[allow(clippy::unwrap_used)]
    fn resume_handler_does_not_block_sibling_completion() {
        let mut engine = engine_from(handle(
            1,
            invoke("./handler.ts", "handler"),
            parallel(vec![
                chain(invoke("./a.ts", "a"), perform(1)),
                invoke("./b.ts", "b"),
            ]),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 2); // A and B

        // Complete A → Perform → handler dispatched (async TS handler).
        let (result, handler_ts) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, None);
        assert_eq!(handler_ts.len(), 1);

        // Complete B while handler is in flight.
        // After refactor: should NOT be stashed.
        let result = engine.complete(ts[1].task_id, json!("b_out")).unwrap();
        assert_eq!(result, None);
        assert_eq!(
            engine.stashed_items.len(),
            0,
            "resume handler should not cause stashing"
        );
    }

    /// Two concurrent resume Performs should both dispatch their handlers
    /// without serialization.
    ///
    /// Setup: Handle(e, ts_handler, All(Perform(e), Perform(e)))
    /// Both Performs fire during advance. Neither should block the other.
    ///
    /// Currently: first Perform suspends Handle, second is stashed.
    /// After refactor: both dispatch concurrently (two handler dispatches).
    #[test]
    #[should_panic(expected = "both handlers should dispatch concurrently")]
    #[allow(clippy::unwrap_used)]
    fn concurrent_resume_performs_not_serialized() {
        let mut engine = engine_from(handle(
            1,
            invoke("./handler.ts", "handler"),
            parallel(vec![perform(1), perform(1)]),
        ));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        // After refactor: both Performs should dispatch, no stashing.
        let ts = engine.take_pending_dispatches();
        assert_eq!(ts.len(), 2, "both handlers should dispatch concurrently");
        assert_eq!(
            engine.stashed_items.len(),
            0,
            "no Perform should be stashed"
        );
    }

    /// A throw (to an outer restart+Branch) should proceed even while a resume
    /// handler is in flight in a sibling branch.
    ///
    /// Setup:
    ///   restart_branch(outer_e,
    ///     Handle(inner_e, ts_handler,             // resume-style (bind)
    ///       All(
    ///         Chain(Invoke(A), Perform(inner_e)), // branch 0: reads var
    ///         Chain(Invoke(B), break_perform(outer_e)), // branch 1: throws
    ///       )
    ///     ),
    ///     identity_action()
    ///   )
    ///
    /// Branch 0: A completes → Perform(inner_e) → resume handler in flight.
    /// Branch 1: B completes → break_perform(outer_e) → should bubble up past
    ///   inner Handle and reach outer Handle. Handler restarts, Branch takes
    ///   Break arm, Handle exits.
    ///
    /// Currently: Perform(outer_e) is stashed because inner Handle is
    ///   suspended. After refactor: inner Handle is not suspended (resume
    ///   handler), so the throw proceeds immediately.
    #[test]
    #[should_panic(expected = "throw should reach outer handler immediately")]
    #[allow(clippy::unwrap_used)]
    fn throw_proceeds_while_resume_handler_in_flight() {
        let inner_e = 1; // resume-style
        let outer_e = 2; // restart-style (tryCatch)

        let inner_handle = handle(
            inner_e,
            invoke("./handler.ts", "handler"), // async resume handler
            parallel(vec![
                chain(invoke("./a.ts", "a"), perform(inner_e)),
                chain(invoke("./b.ts", "b"), break_perform(outer_e)),
            ]),
        );

        let mut engine = engine_from(restart_branch(outer_e, inner_handle, identity_action()));
        let root = engine.workflow_root();
        engine.advance(root, json!("input"), None).unwrap();

        let (_, ts) = drive_builtins(&mut engine).unwrap();
        assert_eq!(ts.len(), 2); // A and B

        // Complete A → Perform(inner_e) → resume handler dispatched.
        let (result, handler_ts) =
            complete_and_drive(&mut engine, ts[0].task_id, json!("a_out")).unwrap();
        assert_eq!(result, None);
        assert_eq!(handler_ts.len(), 1);

        // Complete B → break_perform(outer_e). Should bubble up past inner Handle
        // (which is NOT suspended for a resume handler) and reach outer Handle.
        let (result, _) = complete_and_drive(&mut engine, ts[1].task_id, json!("b_out")).unwrap();

        // Outer handler restarts body, Branch takes Break arm, Handle exits.
        assert_eq!(
            result,
            Some(json!("b_out")),
            "throw should reach outer handler immediately"
        );
    }
}
