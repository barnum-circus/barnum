//! Pure state-machine workflow engine for Barnum.
//!
//! The engine is a synchronous state machine with no I/O, no async, no timers,
//! and no concurrency. External code drives it by calling [`WorkflowState::advance`]
//! and draining dispatches via [`WorkflowState::take_pending_dispatches`].

pub mod frame;

use std::collections::BTreeMap;

use barnum_ast::HandlerKind;
use barnum_ast::flat::{ActionId, FlatAction, FlatConfig, HandlerId};
use frame::{Frame, FrameId, FrameKind, ParentRef};
use intern::Lookup;
use serde_json::Value;
use slab::Slab;
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
    frames: Slab<Frame>,
    task_to_parent: BTreeMap<TaskId, Option<ParentRef>>,
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
            frames: Slab::new(),
            task_to_parent: BTreeMap::new(),
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
        self.deliver(parent, value)
    }

    // -- Private helpers --

    fn insert_frame(&mut self, frame: Frame) -> FrameId {
        FrameId(self.frames.insert(frame))
    }

    #[allow(clippy::missing_const_for_fn)] // mutates self
    fn next_task_id(&mut self) -> TaskId {
        let id = TaskId(self.next_task_id);
        self.next_task_id += 1;
        id
    }

    /// Deliver a value to the parent that was waiting for it.
    ///
    /// - **No parent:** workflow done — return the terminal value.
    /// - **Chain:** trampoline — advance the `rest` action with the value.
    /// - **Loop:** inspect `Continue`/`Break` — re-enter or deliver to parent.
    /// - **Parallel/ForEach:** store in results slot; if all slots filled,
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

        let frame_id = parent_ref.frame_id();

        match parent_ref {
            ParentRef::SingleChild { .. } => {
                let frame = self.frames.remove(frame_id.0);
                match frame.kind {
                    FrameKind::Chain { rest } => {
                        self.advance(rest, value, frame.parent)?;
                        Ok(None)
                    }
                    FrameKind::Loop { body } => match value["kind"].as_str() {
                        Some("Continue") => {
                            let frame_id = self.insert_frame(Frame {
                                parent: frame.parent,
                                kind: FrameKind::Loop { body },
                            });
                            self.advance(
                                body,
                                value["value"].clone(),
                                Some(ParentRef::SingleChild { frame_id }),
                            )?;
                            Ok(None)
                        }
                        Some("Break") => self.deliver(frame.parent, value["value"].clone()),
                        _ => Err(CompleteError::InvalidLoopResult { value }),
                    },
                    _ => unreachable!(
                        "SingleChild parent must be Chain or Loop, got {:?}",
                        frame.kind
                    ),
                }
            }
            ParentRef::IndexedChild { child_index, .. } => {
                let frame = self
                    .frames
                    .get_mut(frame_id.0)
                    .expect("parent frame exists");
                match &mut frame.kind {
                    FrameKind::Parallel { results } | FrameKind::ForEach { results } => {
                        results[child_index] = Some(value);
                        if results.iter().all(Option::is_some) {
                            let collected: Vec<Value> =
                                results.iter_mut().map(|r| r.take().unwrap()).collect();
                            let parent = frame.parent;
                            self.frames.remove(frame_id.0);
                            self.deliver(parent, Value::Array(collected))
                        } else {
                            Ok(None)
                        }
                    }
                    _ => unreachable!(
                        "IndexedChild parent must be Parallel or ForEach, got {:?}",
                        frame.kind
                    ),
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
                self.advance(first, value, Some(ParentRef::SingleChild { frame_id }))?;
            }

            FlatAction::Parallel { count } => {
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
                    kind: FrameKind::Parallel {
                        results: vec![None; count.0 as usize],
                    },
                });
                for (i, child) in children.into_iter().enumerate() {
                    self.advance(
                        child,
                        value.clone(),
                        Some(ParentRef::IndexedChild {
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
                        Some(ParentRef::IndexedChild {
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
                self.advance(body, value, Some(ParentRef::SingleChild { frame_id }))?;
            }

            FlatAction::Step { target } => {
                self.advance(target, value, parent)?;
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
            step_config_schema: None,
            value_schema: None,
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
        Action::Parallel(ParallelAction { actions })
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

    /// Parallel(A, B, C): all 3 dispatched on advance, all receive the same
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

    /// Nested: Chain inside Parallel. Parallel(Chain(A, B), C) -> dispatches A
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

    /// Parallel with empty children: no dispatches, immediate completion.
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

    /// `Parallel(A, B)`: complete both -> workflow done with collected results.
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
}
