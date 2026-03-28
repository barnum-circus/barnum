//! Pure state-machine workflow engine for Barnum.
//!
//! The engine is a synchronous state machine with no I/O, no async, no timers,
//! and no concurrency. External code drives it by calling [`Engine::advance`]
//! and draining dispatches via [`Engine::take_pending_dispatches`].

pub mod frame;

use barnum_ast::HandlerKind;
use barnum_ast::flat::{ActionId, FlatAction, FlatConfig, HandlerId};
use frame::{Frame, FrameId, FrameKind, ParentRef};
use intern::Lookup;
use serde_json::Value;
use slab::Slab;

// ---------------------------------------------------------------------------
// Dispatch
// ---------------------------------------------------------------------------

/// A pending handler invocation produced by `advance`.
#[derive(Debug)]
pub struct Dispatch {
    /// Index into the handler pool. Resolve via [`Engine::handler`].
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
// Engine
// ---------------------------------------------------------------------------

/// Pure state-machine workflow engine.
///
/// Call [`start`](Engine::start) to begin execution, then drain dispatches via
/// [`take_pending_dispatches`](Engine::take_pending_dispatches).
#[derive(Debug)]
pub struct Engine {
    flat_config: FlatConfig,
    frames: Slab<Frame>,
    pending_dispatches: Vec<Dispatch>,
}

impl Engine {
    /// Create a new engine from a flattened config.
    #[must_use]
    pub const fn new(flat_config: FlatConfig) -> Self {
        Self {
            flat_config,
            frames: Slab::new(),
            pending_dispatches: Vec::new(),
        }
    }

    /// The workflow's root action. Pass this to [`advance`](Engine::advance)
    /// with the initial input to start execution.
    #[must_use]
    pub const fn workflow_root(&self) -> ActionId {
        self.flat_config.workflow_root()
    }

    /// Convenience: advance from the workflow root with `parent: None`.
    ///
    /// Equivalent to:
    /// ```ignore
    /// let root = engine.workflow_root();
    /// engine.advance(root, input, None)?;
    /// ```
    ///
    /// # Errors
    ///
    /// Returns [`AdvanceError`] if the workflow encounters a structural error
    /// during expansion (e.g., `ForEach` on a non-array, `Branch` with no
    /// matching case).
    pub fn start(&mut self, input: Value) -> Result<(), AdvanceError> {
        let workflow_root = self.workflow_root();
        self.advance(workflow_root, input, None)
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

    // -- Private helpers --

    fn insert_frame(&mut self, frame: Frame) -> FrameId {
        FrameId(self.frames.insert(frame))
    }

    /// No-op in the advance milestone. The completion milestone fills this in
    /// with the full advance/complete cycle.
    #[allow(clippy::unused_self, clippy::needless_pass_by_ref_mut)]
    fn complete(&mut self, _parent: Option<ParentRef>, _value: Value) {
        // No-op: the value is discarded. Called by advance for empty
        // ForEach/Parallel — the empty result has nowhere to go until
        // completion is implemented.
    }

    /// Expand an `ActionId` into frames. Creates frames for structural
    /// combinators and bottoms out at Invoke leaves with pending dispatches.
    ///
    /// Invoke actions do not create frames — they produce a [`Dispatch`] and
    /// record the parent reference for later delivery via `on_task_completed`.
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
    #[allow(clippy::too_many_lines)]
    pub fn advance(
        &mut self,
        action_id: ActionId,
        value: Value,
        parent: Option<ParentRef>,
    ) -> Result<(), AdvanceError> {
        match self.flat_config.action(action_id) {
            FlatAction::Invoke { handler } => {
                self.pending_dispatches.push(Dispatch {
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
                    self.complete(parent, Value::Array(vec![]));
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
                    self.complete(parent, Value::Array(vec![]));
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

            FlatAction::Attempt { child } => {
                let frame_id = self.insert_frame(Frame {
                    parent,
                    kind: FrameKind::Attempt,
                });
                self.advance(child, value, Some(ParentRef::SingleChild { frame_id }))?;
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

    fn attempt(action: Action) -> Action {
        Action::Attempt(AttemptAction {
            action: Box::new(action),
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
    fn engine_from(workflow: Action) -> Engine {
        let config = Config {
            workflow,
            steps: HashMap::new(),
        };
        Engine::new(flatten(config).unwrap())
    }

    #[allow(clippy::unwrap_used)]
    fn engine_from_config(config: Config) -> Engine {
        Engine::new(flatten(config).unwrap())
    }

    // -- Tests --

    /// Single invoke: start -> 1 dispatch.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn single_invoke() {
        let mut engine = engine_from(invoke("./handler.ts", "run"));
        engine.start(json!({"x": 1})).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].value, json!({"x": 1}));
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./handler.ts", "run"),
        );
    }

    /// Chain(A, B): only A is dispatched on start.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn chain_dispatches_first_only() {
        let mut engine = engine_from(chain(invoke("./a.ts", "a"), invoke("./b.ts", "b")));
        engine.start(json!(null)).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./a.ts", "a"),
        );
    }

    /// Parallel(A, B, C): all 3 dispatched on start, all receive the same
    /// input.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn parallel_dispatches_all() {
        let mut engine = engine_from(parallel(vec![
            invoke("./a.ts", "a"),
            invoke("./b.ts", "b"),
            invoke("./c.ts", "c"),
        ]));
        engine.start(json!({"shared": true})).unwrap();

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
        engine.start(json!([10, 20, 30])).unwrap();

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
        engine.start(json!({"kind": "Ok", "value": 42})).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./ok.ts", "handle"),
        );
    }

    /// Loop: body is dispatched on start.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn loop_dispatches_body() {
        let mut engine = engine_from(loop_action(invoke("./handler.ts", "run")));
        engine.start(json!("init")).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].value, json!("init"));
    }

    /// Attempt: child is dispatched on start.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn attempt_dispatches_child() {
        let mut engine = engine_from(attempt(invoke("./handler.ts", "run")));
        engine.start(json!("input")).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].value, json!("input"));
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
        engine.start(json!(null)).unwrap();

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
        engine.start(json!(null)).unwrap();

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
        engine.start(json!(null)).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./a.ts", "a"),
        );
    }

    /// `ForEach` with empty array: no dispatches.
    /// `complete` is a no-op, so the empty result is discarded.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn foreach_empty_array() {
        let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
        engine.start(json!([])).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 0);
    }

    /// Parallel with empty children: no dispatches.
    /// `complete` is a no-op, so the empty result is discarded.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn parallel_empty() {
        let mut engine = engine_from(parallel(vec![]));
        engine.start(json!(null)).unwrap();

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 0);
    }
}
