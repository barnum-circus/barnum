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
    /// Loop body returned a value that is not `{ kind: "Continue" }` or
    /// `{ kind: "Break" }`.
    #[error("Loop body must return {{kind: \"Continue\"}} or {{kind: \"Break\"}}, got: {value}")]
    InvalidLoopResult {
        /// The invalid value returned by the loop body.
        value: Value,
    },
    /// An advance error occurred during Chain trampoline or Loop re-entry.
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
    /// Discard the continuation. Tear down the body and deliver the value
    /// to the Handle's parent.
    Discard {
        /// The value to deliver to the Handle's parent.
        value: Value,
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
    task_to_parent: BTreeMap<TaskId, Option<ParentRef>>,
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
            task_to_parent: BTreeMap::new(),
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

    /// Deliver a task result. The caller invokes this when a dispatched
    /// handler finishes.
    ///
    /// Returns `Ok(Some(value))` when the workflow terminates, `Ok(None)`
    /// when it's still running.
    ///
    /// # Errors
    ///
    /// Returns [`CompleteError`] if the result value has an invalid shape
    /// (e.g., a Loop body that doesn't return Continue/Break), or if an
    /// advance error occurs during Chain trampoline or Loop re-entry.
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
        let parent = self.task_to_parent.remove(&task_id).expect("unknown task");
        let result = match parent {
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
        // Check each edge starting from the initial parent_ref.
        let mut current_ref = parent_ref;
        loop {
            let Some(frame) = self.frames.get(current_ref.frame_id()) else {
                return AncestorCheck::FrameGone;
            };

            // Does this edge cross into a suspended Handle body?
            if Self::is_blocked_by_handle(&current_ref, &frame.kind) {
                return AncestorCheck::Blocked;
            }

            // Move up to the next edge.
            let Some(next_ref) = frame.parent else {
                return AncestorCheck::Clear;
            };
            current_ref = next_ref;
        }
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
    #[allow(clippy::expect_used)]
    fn find_and_dispatch_handler(
        &mut self,
        starting_parent: ParentRef,
        effect_id: EffectId,
        payload: Value,
    ) -> Result<StashOutcome, AdvanceError> {
        let mut current_frame_id = starting_parent.frame_id();
        loop {
            let frame = self
                .frames
                .get(current_frame_id)
                .expect("ancestor guaranteed present by find_blocking_ancestor");
            if let FrameKind::Handle(handle_frame) = &frame.kind
                && handle_frame.effect_id == effect_id
            {
                // Found the matching Handle. Dispatch to it.
                let perform_parent = starting_parent;
                self.dispatch_to_handler(current_frame_id, perform_parent, payload)?;
                return Ok(StashOutcome::Consumed);
            }
            let Some(next_parent) = frame.parent else {
                return Err(AdvanceError::UnhandledEffect { effect_id });
            };
            current_frame_id = next_parent.frame_id();
        }
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

        // Build the handler input: { payload, state }.
        let handler_input = serde_json::json!({
            "payload": payload,
            "state": state,
        });

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
            HandlerOutput::Discard { value } => self.discard_continuation(handle_frame_id, value),
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
            handle.state = Some(value);
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

    /// Discard the continuation. Tear down the body, deliver to Handle's parent.
    #[allow(clippy::expect_used)]
    fn discard_continuation(
        &mut self,
        handle_frame_id: FrameId,
        value: Value,
    ) -> Result<Option<Value>, CompleteError> {
        self.teardown_body(handle_frame_id);
        let frame = self
            .frames
            .remove(handle_frame_id)
            .expect("Handle frame exists");
        let parent = frame.parent;

        match parent {
            Some(parent_ref) => match self.try_deliver(parent_ref, value)? {
                TryDeliverResult::Delivered(result) => Ok(result),
                TryDeliverResult::Blocked(value) => {
                    self.stashed_items
                        .push_back(StashedItem::Delivery { parent_ref, value });
                    Ok(None)
                }
                TryDeliverResult::FrameGone => Ok(None),
            },
            None => Ok(Some(value)),
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

        // Remove task_to_parent entries pointing into the torn-down subtree.
        self.task_to_parent
            .retain(|_, parent| parent.is_none_or(|p| !to_remove.contains(&p.frame_id())));
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
    /// - **Loop:** inspect `Continue`/`Break` — re-enter or deliver to parent.
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

            ParentRef::Loop { frame_id } => {
                let frame = self.frames.remove(frame_id).expect("parent frame exists");
                let FrameKind::Loop { body } = frame.kind else {
                    unreachable!("Loop ParentRef points to non-Loop frame: {:?}", frame.kind)
                };
                match value["kind"].as_str() {
                    Some("Continue") => {
                        let frame_id = self.insert_frame(Frame {
                            parent: frame.parent,
                            kind: FrameKind::Loop { body },
                        });
                        self.advance(
                            body,
                            value["value"].clone(),
                            Some(ParentRef::Loop { frame_id }),
                        )?;
                        Ok(None)
                    }
                    Some("Break") => self.deliver(frame.parent, value["value"].clone()),
                    _ => Err(CompleteError::InvalidLoopResult { value }),
                }
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
    /// combinators and bottoms out at Invoke leaves with pending dispatches.
    ///
    /// Invoke actions do not create frames — they produce a [`Dispatch`] and
    /// record the parent reference for later delivery via
    /// [`complete`](WorkflowState::complete).
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
                self.task_to_parent.insert(task_id, parent);
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

            FlatAction::Loop { body } => {
                let frame_id = self.insert_frame(Frame {
                    parent,
                    kind: FrameKind::Loop { body },
                });
                self.advance(body, value, Some(ParentRef::Loop { frame_id }))?;
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
                        state: None,
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

    fn loop_action(body: Action) -> Action {
        Action::Loop(LoopAction {
            body: Box::new(body),
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

    /// Loop: body is dispatched on advance.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn loop_dispatches_body() {
        let mut engine = engine_from(loop_action(invoke("./handler.ts", "run")));
        let root = engine.workflow_root();
        engine.advance(root, json!("init"), None).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].value, json!("init"));
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

    /// Loop: Continue re-dispatches, Break completes.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn loop_continue_and_break() {
        let mut engine = engine_from(loop_action(invoke("./handler.ts", "run")));
        let root = engine.workflow_root();
        engine.advance(root, json!(0), None).unwrap();

        // Iteration 1: Continue
        let d = engine.take_pending_dispatches();
        assert_eq!(d[0].value, json!(0));
        assert_eq!(
            engine
                .complete(d[0].task_id, json!({"kind": "Continue", "value": 1}))
                .unwrap(),
            None,
        );

        // Iteration 2: Continue
        let d = engine.take_pending_dispatches();
        assert_eq!(d[0].value, json!(1));
        assert_eq!(
            engine
                .complete(d[0].task_id, json!({"kind": "Continue", "value": 2}))
                .unwrap(),
            None,
        );

        // Iteration 3: Break
        let d = engine.take_pending_dispatches();
        assert_eq!(d[0].value, json!(2));
        assert_eq!(
            engine
                .complete(d[0].task_id, json!({"kind": "Break", "value": "done"}))
                .unwrap(),
            Some(json!("done")),
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
            kind: FrameKind::Loop { body: ActionId(0) },
        });

        // The old id must not resolve, even if the slot was reused.
        assert!(
            arena.get(old_id).is_none(),
            "stale FrameId must not match a reused slot"
        );
    }
}
