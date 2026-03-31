//! Flat representation of the workflow AST.
//!
//! The nested [`Config`](crate::Config) tree is flattened into a [`FlatConfig`]:
//! a linear array of 8-byte entries where all cross-references are index-based.
//! No heap allocation per entry. No side tables — all data is inline.

// Types are named Flat* to distinguish from the tree AST types (Action, Config).
// The module name `flat` is intentionally short.
#![allow(clippy::module_name_repetitions)]

use std::collections::HashMap;
use std::ops::Add;

use u32_newtype::u32_newtype;

use crate::{
    Action, BranchAction, ChainAction, Config, EffectId, HandleAction, HandlerKind, InvokeAction,
    KindDiscriminator, PerformAction, StepName, StepRef,
};

u32_newtype!(
    /// Index into [`FlatConfig::entries`]. Guaranteed to point to an action entry
    /// (not a [`FlatEntry::ChildRef`] or [`FlatEntry::BranchKey`]).
    ActionId
);

u32_newtype!(
    /// Raw position in [`FlatConfig::entries`]. May point to an action, a
    /// [`FlatEntry::ChildRef`], or a [`FlatEntry::BranchKey`]. Produced by
    /// `ActionId + offset` arithmetic when computing child slot positions.
    FlatConfigEntryId
);

u32_newtype!(
    /// Index into [`FlatConfig::handlers`].
    HandlerId
);

u32_newtype!(
    /// Count of children (All) or cases (Branch).
    Count
);

/// `ActionId + offset` yields a `FlatConfigEntryId` (child slot position
/// relative to parent).
impl Add<u32> for ActionId {
    type Output = FlatConfigEntryId;
    fn add(self, offset: u32) -> FlatConfigEntryId {
        FlatConfigEntryId(self.0 + offset)
    }
}

/// `FlatConfigEntryId + offset` yields a `FlatConfigEntryId` (stride within
/// child slots).
impl Add<u32> for FlatConfigEntryId {
    type Output = FlatConfigEntryId;
    fn add(self, offset: u32) -> FlatConfigEntryId {
        FlatConfigEntryId(self.0 + offset)
    }
}

// ---------------------------------------------------------------------------
// FlatAction / FlatEntry
// ---------------------------------------------------------------------------

/// An executable action in the flat table. This is what the interpreter
/// matches on — [`FlatEntry::ChildRef`] and [`FlatEntry::BranchKey`] never
/// appear here.
///
/// Generic over `T`: the Step target type. During pass 1, `T = StepTarget`
/// (may contain unresolved step names). After pass 2, `T = ActionId`
/// (fully resolved). The generic applies only to [`FlatAction::Step`].
///
/// `FlatAction<ActionId>` is `Copy` (all fields are `u32` newtypes).
/// `FlatAction<StepTarget>` is `Clone` but not `Copy` (`StepTarget::Named`
/// holds a `StepName`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatAction<T> {
    /// Leaf: invoke a handler. `handler` indexes [`FlatConfig::handlers`].
    Invoke {
        /// Index into the handler pool.
        handler: HandlerId,
    },

    /// Binary sequential composition: run the child at `action_id + 1`,
    /// then advance to `rest`.
    ///
    /// 2-entry action: the Chain entry itself, followed by one child slot
    /// for `first`. The child slot is either an inlined single-entry action
    /// or a [`FlatEntry::ChildRef`] to a multi-entry subtree elsewhere.
    Chain {
        /// `ActionId` of the continuation (the action to run after `first`).
        rest: ActionId,
    },

    /// Fan-out: same input to all children, collect results as array.
    /// Parent is followed by `count` child slots in the entry array.
    All {
        /// Number of child slots following this entry.
        count: Count,
    },

    /// Map over array input.
    ForEach {
        /// `ActionId` of the loop body.
        body: ActionId,
    },

    /// Case analysis on `value["kind"]`.
    /// Parent is followed by `2 * count` entries: `count` pairs of
    /// (`BranchKey`, child slot).
    Branch {
        /// Number of (key, child) pairs following this entry.
        count: Count,
    },

    /// Redirect to another action (step reference or self-recursion).
    Step {
        /// Target action (unresolved or resolved depending on `T`).
        target: T,
    },

    /// Effect handler. 3-entry action: this entry, then a child slot for
    /// the body (at `action_id + 1`), then a child slot for the handler
    /// (at `action_id + 2`).
    Handle {
        /// Which effect type this handler intercepts.
        effect_id: EffectId,
    },

    /// Raise an effect. Single-entry action (like Invoke). The input
    /// becomes the handler's payload.
    Perform {
        /// Which effect type to raise.
        effect_id: EffectId,
    },
}

impl<T> FlatAction<T> {
    /// Map the Step target through a fallible function. All other variants
    /// pass through unchanged.
    ///
    /// # Errors
    ///
    /// Returns `Err` if `f` returns `Err` for the Step target.
    pub fn try_map_target<U, E>(
        self,
        f: impl FnOnce(T) -> Result<U, E>,
    ) -> Result<FlatAction<U>, E> {
        Ok(match self {
            FlatAction::Step { target } => FlatAction::Step { target: f(target)? },
            FlatAction::Invoke { handler } => FlatAction::Invoke { handler },
            FlatAction::Chain { rest } => FlatAction::Chain { rest },
            FlatAction::All { count } => FlatAction::All { count },
            FlatAction::ForEach { body } => FlatAction::ForEach { body },
            FlatAction::Branch { count } => FlatAction::Branch { count },
            FlatAction::Handle { effect_id } => FlatAction::Handle { effect_id },
            FlatAction::Perform { effect_id } => FlatAction::Perform { effect_id },
        })
    }
}

/// A slot in the entry array. Either an action or inline data
/// (ChildRef/BranchKey).
///
/// Child slots after Chain/All/Branch contain either:
/// - `Action(...)` — a single-entry child inlined directly into the slot
/// - `ChildRef { action }` — a pointer to a multi-entry child elsewhere
///
/// `BranchKey` entries appear in even positions after a Branch; odd positions
/// are child slots.
///
/// Niche optimization: `FlatAction` uses 8 of 256 discriminant values.
/// `ChildRef` and `BranchKey` use 2 more. `FlatEntry<ActionId>` fits in 8 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlatEntry<T> {
    /// An executable action.
    Action(FlatAction<T>),

    /// Child pointer for multi-entry children (Chain/All/Branch).
    /// Points to the root `ActionId` of a child subtree.
    ChildRef {
        /// The `ActionId` of the child subtree root.
        action: ActionId,
    },

    /// Branch case key. Always immediately followed by a child slot.
    BranchKey {
        /// The discriminant key for this branch case.
        key: KindDiscriminator,
    },
}

impl<T> FlatEntry<T> {
    /// Map the Step target through a fallible function. `ChildRef` and
    /// `BranchKey` pass through unchanged.
    ///
    /// # Errors
    ///
    /// Returns `Err` if `f` returns `Err` for the Step target.
    pub fn try_map_target<U, E>(
        self,
        f: impl FnOnce(T) -> Result<U, E>,
    ) -> Result<FlatEntry<U>, E> {
        Ok(match self {
            FlatEntry::Action(action) => FlatEntry::Action(action.try_map_target(f)?),
            FlatEntry::ChildRef { action } => FlatEntry::ChildRef { action },
            FlatEntry::BranchKey { key } => FlatEntry::BranchKey { key },
        })
    }
}

// ---------------------------------------------------------------------------
// StepTarget / FlattenError
// ---------------------------------------------------------------------------

/// Step target during flattening. Named steps are unresolved until pass 2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepTarget {
    /// Unresolved reference to a named step.
    Named(StepName),
    /// Already resolved (e.g. Step(Root) resolved in pass 1).
    Resolved(ActionId),
}

/// Errors that can occur during flattening.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FlattenError {
    /// `Step(Root)` appeared in a step body (only valid in the workflow tree).
    #[error("Step(Root) is only valid in the workflow tree, not in step bodies")]
    StepRootInStepBody,
    /// A named step reference that doesn't exist in `Config::steps`.
    #[error("unknown step: {name}")]
    UnknownStep {
        /// The unresolved step name.
        name: StepName,
    },
    /// A pre-allocated slot was never filled (bug in the flattener).
    #[error("uninitialized entry at index {index}")]
    UninitializedEntry {
        /// The index of the uninitialized slot.
        index: FlatConfigEntryId,
    },
}

// ---------------------------------------------------------------------------
// FlatConfig
// ---------------------------------------------------------------------------

/// The fully-resolved flat configuration.
#[derive(Debug, PartialEq, Eq)]
pub struct FlatConfig {
    /// The entry array. Contains actions (indexed by `ActionId`) and inline
    /// data (`ChildRef`, `BranchKey`) indexed by `FlatConfigEntryId`.
    entries: Vec<FlatEntry<ActionId>>,

    /// Handler pool. `HandlerId`s are indices into this vec.
    /// Handlers are interned: identical handlers share a `HandlerId`.
    handlers: Vec<HandlerKind>,

    /// Entry point for execution.
    workflow_root: ActionId,
}

// Accessors. These enforce structural invariants via panics — a bad ActionId
// indicates a bug in the flattener, not a runtime error.
#[allow(clippy::panic, clippy::missing_panics_doc)]
impl FlatConfig {
    /// The workflow entry point.
    #[must_use]
    pub const fn workflow_root(&self) -> ActionId {
        self.workflow_root
    }

    /// Look up an action by `ActionId`. Panics if the position holds
    /// `ChildRef` or `BranchKey`.
    #[must_use]
    pub fn action(&self, id: ActionId) -> FlatAction<ActionId> {
        match self.entries[id.0 as usize] {
            FlatEntry::Action(action) => action,
            ref other => panic!("ActionId {id:?} does not point to an action: {other:?}"),
        }
    }

    /// Look up a handler by `HandlerId`.
    #[must_use]
    pub fn handler(&self, id: HandlerId) -> &HandlerKind {
        &self.handlers[id.0 as usize]
    }

    /// Resolve a child slot to an `ActionId`.
    /// - Inlined action: the slot position is the `ActionId`.
    /// - `ChildRef`: follow the pointer.
    /// - `BranchKey` in a child slot is a bug.
    #[must_use]
    pub fn resolve_child_slot(&self, slot: FlatConfigEntryId) -> ActionId {
        match self.entries[slot.0 as usize] {
            FlatEntry::Action(_) => ActionId(slot.0),
            FlatEntry::ChildRef { action } => action,
            FlatEntry::BranchKey { .. } => panic!("unexpected BranchKey in child slot at {slot:?}"),
        }
    }

    /// Returns the `first` child `ActionId` for a Chain (resolves the child
    /// slot at `action_id + 1`). The `rest` `ActionId` is stored in the
    /// [`FlatAction::Chain`] variant itself.
    #[must_use]
    pub fn chain_first(&self, id: ActionId) -> ActionId {
        debug_assert!(matches!(self.action(id), FlatAction::Chain { .. }));
        self.resolve_child_slot(id + 1)
    }

    /// Returns the child `ActionId`s for a All.
    pub fn parallel_children(&self, id: ActionId) -> impl Iterator<Item = ActionId> + '_ {
        let count = match self.action(id) {
            FlatAction::All { count } => count.0,
            other => panic!("expected All, got {other:?}"),
        };
        (0..count).map(move |i| self.resolve_child_slot(id + 1 + i))
    }

    /// Returns the body `ActionId` for a Handle (resolves the child
    /// slot at `action_id + 1`).
    #[must_use]
    pub fn handle_body(&self, id: ActionId) -> ActionId {
        debug_assert!(matches!(self.action(id), FlatAction::Handle { .. }));
        self.resolve_child_slot(id + 1)
    }

    /// Returns the handler `ActionId` for a Handle (resolves the child
    /// slot at `action_id + 2`).
    #[must_use]
    pub fn handle_handler(&self, id: ActionId) -> ActionId {
        debug_assert!(matches!(self.action(id), FlatAction::Handle { .. }));
        self.resolve_child_slot(id + 2)
    }

    /// Returns (key, action) pairs for a Branch.
    pub fn branch_cases(
        &self,
        id: ActionId,
    ) -> impl Iterator<Item = (KindDiscriminator, ActionId)> + '_ {
        let count = match self.action(id) {
            FlatAction::Branch { count } => count.0,
            other => panic!("expected Branch, got {other:?}"),
        };
        (0..count).map(move |i| {
            let key_slot = id + 1 + 2 * i;
            let key = match self.entries[key_slot.0 as usize] {
                FlatEntry::BranchKey { key } => key,
                ref other => panic!("expected BranchKey at {key_slot:?}, got {other:?}"),
            };
            let child_slot = key_slot + 1;
            (key, self.resolve_child_slot(child_slot))
        })
    }
}

// ---------------------------------------------------------------------------
// UnresolvedFlatConfig (builder)
// ---------------------------------------------------------------------------

/// The builder — the unresolved version of [`FlatConfig`]. Holds the entry
/// array with `Option` placeholders for pre-allocated slots and the handler
/// interning state.
struct UnresolvedFlatConfig {
    entries: Vec<Option<FlatEntry<StepTarget>>>,
    handlers: Vec<HandlerKind>,
}

#[allow(clippy::cast_possible_truncation)]
impl UnresolvedFlatConfig {
    const fn new() -> Self {
        Self {
            entries: Vec::new(),
            handlers: Vec::new(),
        }
    }

    /// Allocate a single slot for an action.
    fn alloc(&mut self) -> ActionId {
        let id = ActionId(self.entries.len() as u32);
        self.entries.push(None);
        id
    }

    /// Pre-allocate `count` contiguous `None` slots (for child slots).
    fn alloc_many(&mut self, count: Count) {
        self.entries
            .extend(std::iter::repeat_n(None, count.0 as usize));
    }

    /// Intern a handler, returning its `HandlerId`. Identical handlers
    /// (by `PartialEq`) share the same id.
    fn intern_handler(&mut self, handler: HandlerKind) -> HandlerId {
        if let Some(index) = self.handlers.iter().position(|h| h == &handler) {
            return HandlerId(index as u32);
        }
        let index = self.handlers.len();
        self.handlers.push(handler);
        HandlerId(index as u32)
    }

    /// Allocate a slot, flatten an action into it, return its `ActionId`.
    fn flatten_action(
        &mut self,
        action: Action,
        workflow_root: Option<ActionId>,
    ) -> Result<ActionId, FlattenError> {
        let action_id = self.alloc();
        self.flatten_action_at(action, action_id, workflow_root)?;
        Ok(action_id)
    }

    /// Write an action's root entry into an existing slot. The single match
    /// over all `Action` variants — no duplication.
    ///
    /// For Chain/All/Branch, child slots are allocated immediately
    /// after the slot. This means the slot must be at the end of the vec
    /// for multi-entry actions (guaranteed when called from `flatten_action`).
    fn flatten_action_at(
        &mut self,
        action: Action,
        action_id: ActionId,
        workflow_root: Option<ActionId>,
    ) -> Result<(), FlattenError> {
        let entry = match action {
            Action::Invoke(InvokeAction { handler }) => {
                let handler_id = self.intern_handler(handler);
                FlatAction::Invoke {
                    handler: handler_id,
                }
            }

            Action::Chain(ChainAction { first, rest }) => {
                self.alloc(); // child slot for first
                let rest_action_id = self.flatten_action(*rest, workflow_root)?;
                self.fill_child_slot(*first, action_id + 1, workflow_root)?;
                FlatAction::Chain {
                    rest: rest_action_id,
                }
            }

            Action::All(crate::AllAction { actions }) => {
                let count = Count(actions.len() as u32);
                self.alloc_many(count);
                self.fill_child_slots(actions, action_id + 1, workflow_root)?;
                FlatAction::All { count }
            }

            Action::Branch(BranchAction { cases }) => {
                let count = Count(cases.len() as u32);
                let mut cases: Vec<_> = cases.into_iter().collect();
                cases.sort_by_key(|(key, _)| *key);
                self.alloc_many(Count(2 * count.0));
                for (i, (key, child)) in cases.into_iter().enumerate() {
                    let key_slot = action_id + 1 + 2 * i as u32;
                    self.entries[key_slot.0 as usize] = Some(FlatEntry::BranchKey { key });
                    self.fill_child_slot(child, key_slot + 1, workflow_root)?;
                }
                FlatAction::Branch { count }
            }

            Action::ForEach(crate::ForEachAction { action }) => {
                let body_id = self.flatten_action(*action, workflow_root)?;
                FlatAction::ForEach { body: body_id }
            }

            Action::Step(crate::StepAction {
                step: StepRef::Named { name },
            }) => FlatAction::Step {
                target: StepTarget::Named(name),
            },

            Action::Step(crate::StepAction {
                step: StepRef::Root,
            }) => {
                let root = workflow_root.ok_or(FlattenError::StepRootInStepBody)?;
                FlatAction::Step {
                    target: StepTarget::Resolved(root),
                }
            }

            Action::Handle(HandleAction {
                effect_id,
                body,
                handler,
            }) => {
                self.alloc(); // child slot for body (at action_id + 1)
                self.alloc(); // child slot for handler (at action_id + 2)
                self.fill_child_slot(*body, action_id + 1, workflow_root)?;
                self.fill_child_slot(*handler, action_id + 2, workflow_root)?;
                FlatAction::Handle { effect_id }
            }

            Action::Perform(PerformAction { effect_id }) => FlatAction::Perform { effect_id },
        };
        self.entries[action_id.0 as usize] = Some(FlatEntry::Action(entry));
        Ok(())
    }

    /// Fill a child slot with an action. Single-entry actions are inlined
    /// directly into the slot (the `FlatConfigEntryId` becomes an `ActionId`).
    /// Multi-entry actions (Chain/All/Branch) are flattened elsewhere
    /// via `flatten_action`, and a `ChildRef` is written into the slot.
    fn fill_child_slot(
        &mut self,
        action: Action,
        slot: FlatConfigEntryId,
        workflow_root: Option<ActionId>,
    ) -> Result<(), FlattenError> {
        match action {
            Action::Chain { .. }
            | Action::All { .. }
            | Action::Branch { .. }
            | Action::Handle { .. } => {
                let action_id = self.flatten_action(action, workflow_root)?;
                self.entries[slot.0 as usize] = Some(FlatEntry::ChildRef { action: action_id });
            }
            single_entry => {
                // Inline: this child slot IS the action. Convert to ActionId.
                self.flatten_action_at(single_entry, ActionId(slot.0), workflow_root)?;
            }
        }
        Ok(())
    }

    /// Fill contiguous child slots from a `Vec<Action>`.
    fn fill_child_slots(
        &mut self,
        actions: Vec<Action>,
        start: FlatConfigEntryId,
        workflow_root: Option<ActionId>,
    ) -> Result<(), FlattenError> {
        for (i, action) in actions.into_iter().enumerate() {
            self.fill_child_slot(action, start + i as u32, workflow_root)?;
        }
        Ok(())
    }

    /// Resolve step names and produce the final [`FlatConfig`].
    fn resolve(
        self,
        workflow_root: ActionId,
        step_roots: &HashMap<StepName, ActionId>,
    ) -> Result<FlatConfig, FlattenError> {
        let entries = self
            .entries
            .into_iter()
            .enumerate()
            .map(|(i, slot)| {
                let entry = slot.ok_or(FlattenError::UninitializedEntry {
                    index: FlatConfigEntryId(i as u32),
                })?;
                entry.try_map_target(|target| match target {
                    StepTarget::Named(name) => step_roots
                        .get(&name)
                        .copied()
                        .ok_or(FlattenError::UnknownStep { name }),
                    StepTarget::Resolved(id) => Ok(id),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;

        Ok(FlatConfig {
            entries,
            handlers: self.handlers,
            workflow_root,
        })
    }
}

// ---------------------------------------------------------------------------
// Top-level flatten
// ---------------------------------------------------------------------------

/// Flatten a [`Config`] into a [`FlatConfig`].
///
/// All errors from pass 1 (DFS flattening) and pass 2 (step resolution) are
/// returned. In practice, config validation catches these issues before
/// flattening, so errors here indicate a bug.
///
/// # Errors
///
/// Returns [`FlattenError::StepRootInStepBody`] if `Step(Root)` appears in
/// a step body. Returns [`FlattenError::UnknownStep`] if a named step
/// reference doesn't exist. Returns [`FlattenError::UninitializedEntry`]
/// if a pre-allocated slot was never filled (flattener bug).
#[allow(clippy::cast_possible_truncation)]
pub fn flatten(config: Config) -> Result<FlatConfig, FlattenError> {
    let mut unresolved_flat_config = UnresolvedFlatConfig::new();

    // The workflow root will be at the next alloc position.
    let workflow_root = ActionId(unresolved_flat_config.entries.len() as u32);
    unresolved_flat_config.flatten_action(config.workflow, Some(workflow_root))?;

    // Sort steps by name for deterministic ActionId assignment.
    let mut steps: Vec<_> = config.steps.into_iter().collect();
    steps.sort_by_key(|(name, _)| *name);

    let mut step_roots = HashMap::new();
    for (name, step_action) in steps {
        let step_root = unresolved_flat_config.flatten_action(step_action, None)?;
        step_roots.insert(name, step_root);
    }

    unresolved_flat_config.resolve(workflow_root, &step_roots)
}

// ---------------------------------------------------------------------------
// Static assertions
// ---------------------------------------------------------------------------

const _: () = assert!(std::mem::size_of::<FlatEntry<ActionId>>() <= 8);

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use intern::string_key::Intern;

    /// Helper: create a `StepName` from a string literal.
    fn step_name(s: &str) -> StepName {
        StepName::from(s.intern())
    }

    /// Helper: create a `KindDiscriminator` from a string literal.
    fn kind(s: &str) -> KindDiscriminator {
        KindDiscriminator::from(s.intern())
    }

    /// Helper: create a simple TypeScript handler with the given module and func.
    fn ts_handler(module: &str, func: &str) -> HandlerKind {
        use crate::{FuncName, ModulePath, TypeScriptHandler};
        HandlerKind::TypeScript(TypeScriptHandler {
            module: ModulePath::from(module.intern()),
            func: FuncName::from(func.intern()),
        })
    }

    /// Helper: create an Invoke action.
    fn invoke(module: &str, func: &str) -> Action {
        Action::Invoke(InvokeAction {
            handler: ts_handler(module, func),
        })
    }

    /// Helper: right-fold actions into nested Chain nodes (mirrors TS `pipe()`).
    #[allow(clippy::panic, clippy::unwrap_used)]
    fn pipe(actions: Vec<Action>) -> Action {
        match actions.len() {
            0 => panic!("pipe() with zero actions not supported in tests"),
            _ => actions
                .into_iter()
                .rev()
                .reduce(|rest, first| {
                    Action::Chain(ChainAction {
                        first: Box::new(first),
                        rest: Box::new(rest),
                    })
                })
                .unwrap(),
        }
    }

    /// Helper: create a All action.
    fn parallel(actions: Vec<Action>) -> Action {
        Action::All(crate::AllAction { actions })
    }

    /// Helper: create a `ForEach` action.
    fn for_each(action: Action) -> Action {
        Action::ForEach(crate::ForEachAction {
            action: Box::new(action),
        })
    }

    /// Helper: create a Branch action.
    fn branch(cases: Vec<(&str, Action)>) -> Action {
        Action::Branch(BranchAction {
            cases: cases.into_iter().map(|(k, v)| (kind(k), v)).collect(),
        })
    }

    /// Helper: create a Step(Named) action.
    fn step_named(name: &str) -> Action {
        Action::Step(crate::StepAction {
            step: StepRef::Named {
                name: step_name(name),
            },
        })
    }

    /// Helper: create a Step(Root) action.
    fn step_root() -> Action {
        Action::Step(crate::StepAction {
            step: StepRef::Root,
        })
    }

    /// Helper: create a Config with no steps.
    fn config(workflow: Action) -> Config {
        Config {
            workflow,
            steps: HashMap::new(),
        }
    }

    /// Helper: create a Config with steps.
    fn config_with_steps(workflow: Action, steps: Vec<(&str, Action)>) -> Config {
        Config {
            workflow,
            steps: steps
                .into_iter()
                .map(|(name, action)| (step_name(name), action))
                .collect(),
        }
    }

    // -- Basic structure --

    /// Single invoke: one entry, root = 0.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_single_invoke() {
        let flat = flatten(config(invoke("./handler.ts", "run"))).unwrap();
        assert_eq!(flat.workflow_root(), ActionId(0));
        assert_eq!(flat.entries.len(), 1);
        assert_eq!(
            flat.action(ActionId(0)),
            FlatAction::Invoke {
                handler: HandlerId(0)
            }
        );
        assert_eq!(
            flat.handler(HandlerId(0)),
            &ts_handler("./handler.ts", "run")
        );
    }

    /// Chain: `pipe(A, B, C)` → `Chain(A, Chain(B, C))`, right-nested.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_chain() {
        // pipe(A, B, C) → Chain(A, Chain(B, C))
        let flat = flatten(config(pipe(vec![
            invoke("./a.ts", "a"),
            invoke("./b.ts", "b"),
            invoke("./c.ts", "c"),
        ])))
        .unwrap();

        // 0: Chain { rest: 2 }
        // 1: Invoke(0)         ← A (child slot, inlined)
        // 2: Chain { rest: 4 } ← inner Chain (rest of outer)
        // 3: Invoke(1)         ← B (child slot, inlined)
        // 4: Invoke(2)         ← C (rest of inner Chain)
        assert_eq!(flat.entries.len(), 5);
        assert_eq!(
            flat.action(ActionId(0)),
            FlatAction::Chain { rest: ActionId(2) }
        );
        assert_eq!(flat.chain_first(ActionId(0)), ActionId(1));
        assert_eq!(
            flat.action(ActionId(2)),
            FlatAction::Chain { rest: ActionId(4) }
        );
        assert_eq!(flat.chain_first(ActionId(2)), ActionId(3));
    }

    /// All: fan-out with `count` child slots.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_parallel() {
        let flat = flatten(config(parallel(vec![
            invoke("./a.ts", "a"),
            invoke("./b.ts", "b"),
        ])))
        .unwrap();

        assert_eq!(flat.entries.len(), 3);
        assert_eq!(
            flat.action(ActionId(0)),
            FlatAction::All { count: Count(2) }
        );

        let children: Vec<_> = flat.parallel_children(ActionId(0)).collect();
        assert_eq!(children, vec![ActionId(1), ActionId(2)]);
    }

    /// `ForEach`: explicit body `ActionId`.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_foreach() {
        let flat = flatten(config(for_each(invoke("./handler.ts", "run")))).unwrap();

        // ForEach allocates first, then flatten_action for the body allocates second.
        // entries: [ForEach { body: 1 }, Invoke(0)]
        assert_eq!(flat.entries.len(), 2);
        assert_eq!(
            flat.action(ActionId(0)),
            FlatAction::ForEach { body: ActionId(1) }
        );
        assert_eq!(
            flat.action(ActionId(1)),
            FlatAction::Invoke {
                handler: HandlerId(0)
            }
        );
    }

    /// Branch: `BranchKey` + inlined child pairs.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_branch() {
        let flat = flatten(config(branch(vec![
            ("Ok", invoke("./ok.ts", "handle")),
            ("Err", invoke("./err.ts", "handle")),
        ])))
        .unwrap();

        // entries: [Branch{2}, BranchKey("Err"), Invoke(0), BranchKey("Ok"), Invoke(1)]
        // (sorted by key: Err < Ok)
        assert_eq!(flat.entries.len(), 5);
        assert_eq!(
            flat.action(ActionId(0)),
            FlatAction::Branch { count: Count(2) }
        );

        let cases: Vec<_> = flat.branch_cases(ActionId(0)).collect();
        assert_eq!(cases.len(), 2);
        assert_eq!(cases[0].0, kind("Err"));
        assert_eq!(cases[1].0, kind("Ok"));
    }

    // -- Nesting --

    /// Chain with multi-entry first: first is All → `ChildRef`.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_chain_with_multi_entry_first() {
        // Chain(All(X, Y), B) — first is multi-entry, gets ChildRef.
        let action = pipe(vec![
            parallel(vec![invoke("./x.ts", "x"), invoke("./y.ts", "y")]),
            invoke("./b.ts", "b"),
        ]);
        let flat = flatten(config(action)).unwrap();

        // 0: Chain { rest: 2 }
        // 1: ChildRef { action: 3 }   ← first is multi-entry All
        // 2: Invoke(handler_b)         ← rest
        // 3: All { count: 2 }
        // 4: Invoke(handler_x)
        // 5: Invoke(handler_y)
        assert_eq!(flat.entries.len(), 6);
        assert_eq!(
            flat.action(ActionId(0)),
            FlatAction::Chain { rest: ActionId(2) }
        );
        // first is via ChildRef → All at ActionId(3).
        assert_eq!(flat.chain_first(ActionId(0)), ActionId(3));
        assert_eq!(
            flat.action(ActionId(3)),
            FlatAction::All { count: Count(2) }
        );
    }

    /// Branch cases containing compound subtrees.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_branch_with_subtrees() {
        let ok_chain = pipe(vec![invoke("./a.ts", "a"), invoke("./b.ts", "b")]);
        let action = branch(vec![
            ("Ok", ok_chain),
            ("Err", invoke("./err.ts", "handle")),
        ]);
        let flat = flatten(config(action)).unwrap();

        let cases: Vec<_> = flat.branch_cases(ActionId(0)).collect();
        assert_eq!(cases.len(), 2);

        // Err case child is single-entry (Invoke), inlined.
        // Ok case child is multi-entry (Chain), ChildRef.
        for (key, action_id) in &cases {
            let action = flat.action(*action_id);
            if *key == kind("Err") {
                assert!(matches!(action, FlatAction::Invoke { .. }));
            } else {
                assert!(matches!(action, FlatAction::Chain { .. }));
            }
        }
    }

    /// All containing Alls (`ChildRef` for each).
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_parallel_of_parallels() {
        let inner1 = parallel(vec![invoke("./a.ts", "a"), invoke("./b.ts", "b")]);
        let inner2 = parallel(vec![invoke("./c.ts", "c"), invoke("./d.ts", "d")]);
        let action = parallel(vec![inner1, inner2]);
        let flat = flatten(config(action)).unwrap();

        assert_eq!(
            flat.action(ActionId(0)),
            FlatAction::All { count: Count(2) }
        );

        let children: Vec<_> = flat.parallel_children(ActionId(0)).collect();
        assert_eq!(children.len(), 2);
        // Both children are multi-entry Alls → ChildRefs.
        for child in children {
            assert!(matches!(flat.action(child), FlatAction::All { .. }));
        }
    }

    // -- Step resolution --

    /// `Step(Root)` resolved immediately to workflow root `ActionId`.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_step_root() {
        // Chain(Invoke, Step(Root)) — the Step(Root) points back to the Chain.
        let action = pipe(vec![invoke("./a.ts", "a"), step_root()]);
        let flat = flatten(config(action)).unwrap();

        // 0: Chain { rest: 2 }
        // 1: Invoke(0)                ← first (child slot, inlined)
        // 2: Step { target: 0 }       ← rest, points back to workflow root
        assert_eq!(
            flat.action(ActionId(0)),
            FlatAction::Chain { rest: ActionId(2) }
        );
        assert_eq!(
            flat.action(ActionId(2)),
            FlatAction::Step {
                target: ActionId(0)
            }
        );
    }

    /// Named step resolved in pass 2.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_step_named() {
        let action = step_named("Cleanup");
        let cleanup = invoke("./cleanup.ts", "run");
        let flat = flatten(config_with_steps(action, vec![("Cleanup", cleanup)])).unwrap();

        // Workflow: Step(Named("Cleanup")) at 0.
        // Step body: Invoke at 1.
        assert_eq!(
            flat.action(ActionId(0)),
            FlatAction::Step {
                target: ActionId(1)
            }
        );
        assert_eq!(
            flat.action(ActionId(1)),
            FlatAction::Invoke {
                handler: HandlerId(0)
            }
        );
    }

    /// Mutual recursion: A -> B -> A, no infinite loop in flattening.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_mutual_recursion() {
        let flat = flatten(config_with_steps(
            step_named("A"),
            vec![("A", step_named("B")), ("B", step_named("A"))],
        ))
        .unwrap();

        // Workflow: Step(A) at 0.
        // Step A body: Step(B) at 1.
        // Step B body: Step(A) at 2.
        // After resolution, all targets are ActionIds.
        assert_eq!(
            flat.action(ActionId(0)),
            FlatAction::Step {
                target: ActionId(1)
            }
        );
        assert_eq!(
            flat.action(ActionId(1)),
            FlatAction::Step {
                target: ActionId(2)
            }
        );
        assert_eq!(
            flat.action(ActionId(2)),
            FlatAction::Step {
                target: ActionId(1)
            }
        );
    }

    /// Chain of steps: A -> B -> C -> Invoke.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_chain_of_steps() {
        let flat = flatten(config_with_steps(
            step_named("A"),
            vec![
                ("A", step_named("B")),
                ("B", step_named("C")),
                ("C", invoke("./handler.ts", "run")),
            ],
        ))
        .unwrap();

        // All Step targets should resolve.
        assert!(matches!(flat.action(ActionId(0)), FlatAction::Step { .. }));
    }

    // -- Edge cases --

    /// Single-case branch: `BranchKey` + inlined child.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_single_case_branch() {
        let flat = flatten(config(branch(vec![("Ok", invoke("./ok.ts", "handle"))]))).unwrap();

        assert_eq!(flat.entries.len(), 3); // Branch + BranchKey + Invoke
        let cases: Vec<_> = flat.branch_cases(ActionId(0)).collect();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].0, kind("Ok"));
    }

    /// Unknown step name returns Err(UnknownStep).
    #[test]
    fn flatten_unknown_step_errors() {
        let result = flatten(config(step_named("DoesNotExist")));
        assert!(matches!(result, Err(FlattenError::UnknownStep { .. })));
    }

    /// Deterministic: flatten twice, assert identical.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_deterministic() {
        let make_config = || {
            config(pipe(vec![
                invoke("./a.ts", "a"),
                branch(vec![
                    ("Ok", invoke("./ok.ts", "handle")),
                    ("Err", invoke("./err.ts", "handle")),
                ]),
                for_each(invoke("./loop.ts", "body")),
            ]))
        };
        let flat1 = flatten(make_config()).unwrap();
        let flat2 = flatten(make_config()).unwrap();
        assert_eq!(flat1, flat2);
    }

    /// Handler interning: identical handlers share the same `HandlerId`.
    #[test]
    #[allow(clippy::unwrap_used)]
    fn flatten_handler_interning() {
        // pipe(A, A, B) → Chain(A, Chain(A, B))
        // rest is flattened before first, so B is interned first.
        // 0: Chain { rest: 2 }
        // 1: Invoke(1)         ← first A (handler interned second)
        // 2: Chain { rest: 4 }
        // 3: Invoke(1)         ← second A (same handler, interned)
        // 4: Invoke(0)         ← B (handler interned first — deepest rest)
        let flat = flatten(config(pipe(vec![
            invoke("./handler.ts", "run"),
            invoke("./handler.ts", "run"), // same handler
            invoke("./other.ts", "run"),   // different handler
        ])))
        .unwrap();

        // Both A invokes share the same HandlerId.
        let a1 = flat.action(ActionId(1));
        let a2 = flat.action(ActionId(3));
        assert_eq!(a1, a2);

        // B has a different HandlerId.
        let b = flat.action(ActionId(4));
        assert_ne!(a1, b);
    }

    /// Static assert: `FlatEntry<ActionId>` fits in 8 bytes.
    #[test]
    fn flat_entry_size() {
        assert!(std::mem::size_of::<FlatEntry<ActionId>>() <= 8);
    }

    // -- Structural invariants --

    /// `action()` panics when given an `ActionId` that points to a `ChildRef`.
    #[test]
    #[should_panic(expected = "does not point to an action")]
    fn action_rejects_childref() {
        let flat = FlatConfig {
            entries: vec![FlatEntry::ChildRef {
                action: ActionId(0),
            }],
            handlers: vec![],
            workflow_root: ActionId(0),
        };
        let _ = flat.action(ActionId(0));
    }

    /// `action()` panics when given an `ActionId` that points to a `BranchKey`.
    #[test]
    #[should_panic(expected = "does not point to an action")]
    fn action_rejects_branchkey() {
        let flat = FlatConfig {
            entries: vec![FlatEntry::BranchKey { key: kind("test") }],
            handlers: vec![],
            workflow_root: ActionId(0),
        };
        let _ = flat.action(ActionId(0));
    }

    /// `resolve_child_slot` panics on `BranchKey`.
    #[test]
    #[should_panic(expected = "unexpected BranchKey")]
    fn resolve_child_slot_rejects_branchkey() {
        let flat = FlatConfig {
            entries: vec![FlatEntry::BranchKey { key: kind("test") }],
            handlers: vec![],
            workflow_root: ActionId(0),
        };
        let _ = flat.resolve_child_slot(FlatConfigEntryId(0));
    }

    /// Step(Root) in a step body returns Err(StepRootInStepBody).
    #[test]
    fn step_root_in_step_body_errors() {
        let result = flatten(config_with_steps(
            invoke("./handler.ts", "run"),
            vec![("Bad", step_root())],
        ));
        assert!(matches!(result, Err(FlattenError::StepRootInStepBody)));
    }
}
