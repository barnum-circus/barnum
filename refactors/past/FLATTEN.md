# Flattening

The nested `Config` (tree of `Action` nodes with step references by name) is flattened into a `FlatConfig`: a linear array of entries where all cross-references are `ActionId` indices. No heap allocation. No side tables — all data is inline in the entries vec.

## Types

**Note:** `ActionId`, `FlatConfigEntryId`, `HandlerId`, and `Count` are `u32` newtypes via a `u32_newtype!` macro (modeled after isograph's `u64_newtypes` crate). To be created when implementation starts.

**`ActionId` vs `FlatConfigEntryId`:** Both are indices into `FlatConfig::entries`. `ActionId` is guaranteed to point to an action entry (Invoke, Pipe, Parallel, ForEach, Branch, Loop, Attempt, Step). `FlatConfigEntryId` is a raw position that might also be a ChildRef or BranchKey — produced by `ActionId + u32` arithmetic when computing child slot positions relative to a parent. `resolve_child_slot(FlatConfigEntryId) -> ActionId` resolves the indirection.

```rust
/// ActionId + offset = FlatConfigEntryId (child slot position relative to parent).
impl Add<u32> for ActionId {
    type Output = FlatConfigEntryId;
    fn add(self, offset: u32) -> FlatConfigEntryId {
        FlatConfigEntryId(self.0 + offset)
    }
}

/// FlatConfigEntryId + offset = FlatConfigEntryId (stride within child slots).
impl Add<u32> for FlatConfigEntryId {
    type Output = FlatConfigEntryId;
    fn add(self, offset: u32) -> FlatConfigEntryId {
        FlatConfigEntryId(self.0 + offset)
    }
}
```

```rust
/// An executable action in the flat table. This is what the interpreter
/// matches on — ChildRef and BranchKey never appear here.
///
/// Generic over `T`: the Step target type. During pass 1, `T = StepTarget`
/// (may contain unresolved step names). After pass 2, `T = ActionId`
/// (fully resolved). The generic applies only to Step.
///
/// `FlatAction<ActionId>` is Copy (all fields are u32 newtypes).
/// `FlatAction<StepTarget>` is Clone but not Copy (StepTarget::Named holds a String).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlatAction<T> {
    /// Leaf: invoke a handler. `handler` indexes `FlatConfig::handlers`.
    Invoke { handler: HandlerId },

    /// Sequential composition.
    /// Parent is followed by `count` child slots in the entry array.
    Pipe { count: Count },

    /// Fan-out: same input to all children, collect results as array.
    /// Parent is followed by `count` child slots in the entry array.
    Parallel { count: Count },

    /// Map over array input.
    ForEach { body: ActionId },

    /// Case analysis on value["kind"].
    /// Parent is followed by `2 * count` entries: `count` pairs of
    /// (BranchKey, child slot).
    Branch { count: Count },

    /// Loop: runs body, inspects result variant to break or continue.
    Loop { body: ActionId },

    /// Error materialization.
    Attempt { child: ActionId },

    /// Redirect to another action (step reference or self-recursion).
    Step { target: T },
}

/// A slot in the entry array. Either an action or inline data (ChildRef/BranchKey).
///
/// Child slots after Pipe/Parallel/Branch contain either:
/// - `Action(...)` — a single-entry child inlined directly into the slot
/// - `ChildRef { action }` — a pointer to a multi-entry child elsewhere
///
/// BranchKey entries appear in even positions after a Branch; odd positions
/// are child slots.
///
/// Niche optimization: FlatAction uses 8 of 256 discriminant values.
/// ChildRef and BranchKey use 2 more. FlatEntry<ActionId> fits in 8 bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlatEntry<T> {
    Action(FlatAction<T>),

    /// Child pointer for multi-entry children (Pipe/Parallel/Branch).
    /// Points to the root ActionId of a child subtree.
    ChildRef { action: ActionId },

    /// Branch case key. Always immediately followed by a child slot.
    BranchKey { key: KindDiscriminator },
}
```

Every `FlatAction` variant has at most one `u32` field. `FlatEntry` wraps it with two additional variants (ChildRef, BranchKey), each also carrying one `u32`. Niche optimization ensures `FlatEntry<ActionId>` remains 8 bytes. Enforce with `static_assert!(size_of::<FlatEntry<ActionId>>() <= 8)`.

**Future optimization:** Branch cases currently use two entries each (BranchKey + child slot). If KindDiscriminator is narrowed to `u16`, a single `BranchCase { key: u16, action: ActionId }` variant (6 bytes payload, fits in 8 with discriminant) could halve branch overhead.

```rust
impl<T> FlatAction<T> {
    fn try_map_target<U, E>(self, f: impl FnOnce(T) -> Result<U, E>) -> Result<FlatAction<U>, E> {
        Ok(match self {
            FlatAction::Step { target } => FlatAction::Step { target: f(target)? },
            FlatAction::Invoke { handler } => FlatAction::Invoke { handler },
            FlatAction::Pipe { count } => FlatAction::Pipe { count },
            FlatAction::Parallel { count } => FlatAction::Parallel { count },
            FlatAction::ForEach { body } => FlatAction::ForEach { body },
            FlatAction::Branch { count } => FlatAction::Branch { count },
            FlatAction::Loop { body } => FlatAction::Loop { body },
            FlatAction::Attempt { child } => FlatAction::Attempt { child },
        })
    }
}

impl<T> FlatEntry<T> {
    fn try_map_target<U, E>(self, f: impl FnOnce(T) -> Result<U, E>) -> Result<FlatEntry<U>, E> {
        Ok(match self {
            FlatEntry::Action(action) => FlatEntry::Action(action.try_map_target(f)?),
            FlatEntry::ChildRef { action } => FlatEntry::ChildRef { action },
            FlatEntry::BranchKey { key } => FlatEntry::BranchKey { key },
        })
    }
}

enum FlattenError {
    StepRootInStepBody,
    UnknownStep { name: StepName },
    UninitializedEntry { index: FlatConfigEntryId },
}
```

```rust
enum StepTarget {
    Named(StepName),
    Resolved(ActionId),
}
```

```rust
/// The fully-resolved flat configuration.
#[derive(Debug, PartialEq, Eq)]
struct FlatConfig {
    /// The entry array. Contains actions (indexed by ActionId) and inline data
    /// (ChildRef, BranchKey) indexed by FlatConfigEntryId. Use `action()` to look up
    /// an ActionId; use `resolve_child_slot()` to resolve a FlatConfigEntryId.
    entries: Vec<FlatEntry<ActionId>>,

    /// Handler pool. HandlerIds are indices into this vec.
    /// Handlers are interned: identical handlers share a HandlerId.
    handlers: Vec<HandlerKind>,

    /// Entry point for execution.
    workflow_root: ActionId,
}
```

### Accessors

The interpreter works with `ActionId` and `FlatAction`. Child slots (positions after Pipe/Parallel/Branch) are `FlatConfigEntryId`s — resolve them via `resolve_child_slot` to get an `ActionId`, then call `action()` to get the `FlatAction`.

```rust
impl FlatConfig {
    /// Look up an action by ActionId. Panics if the position holds
    /// ChildRef or BranchKey — those are inline data, not actions.
    fn action(&self, id: ActionId) -> FlatAction<ActionId> {
        match self.entries[id.0 as usize] {
            FlatEntry::Action(action) => action,
            other => panic!("ActionId {id:?} does not point to an action: {other:?}"),
        }
    }

    fn handler(&self, id: HandlerId) -> &HandlerKind {
        &self.handlers[id.0 as usize]
    }

    /// Resolve a child slot to an ActionId.
    /// - Inlined action: the slot position is the ActionId.
    /// - ChildRef: follow the pointer.
    /// - BranchKey in a child slot is a bug.
    fn resolve_child_slot(&self, slot: FlatConfigEntryId) -> ActionId {
        match self.entries[slot.0 as usize] {
            FlatEntry::Action(_) => ActionId(slot.0),
            FlatEntry::ChildRef { action } => action,
            FlatEntry::BranchKey { .. } => panic!("unexpected BranchKey in child slot"),
        }
    }

    /// Returns the child ActionIds for a Pipe or Parallel.
    fn children(&self, id: ActionId) -> impl Iterator<Item = ActionId> + '_ {
        let count = match self.action(id) {
            FlatAction::Pipe { count } | FlatAction::Parallel { count } => count.0,
            other => panic!("expected Pipe or Parallel, got {other:?}"),
        };
        (0..count).map(move |i| self.resolve_child_slot(id + 1 + i))
    }

    /// Returns (key, action) pairs for a Branch.
    fn branch_cases(&self, id: ActionId) -> impl Iterator<Item = (KindDiscriminator, ActionId)> + '_ {
        let count = match self.action(id) {
            FlatAction::Branch { count } => count.0,
            other => panic!("expected Branch, got {other:?}"),
        };
        (0..count).map(move |i| {
            let key_slot = id + 1 + 2 * i;
            let key = match self.entries[key_slot.0 as usize] {
                FlatEntry::BranchKey { key } => key,
                other => panic!("expected BranchKey at {key_slot:?}, got {other:?}"),
            };
            let child_slot = key_slot + 1;
            (key, self.resolve_child_slot(child_slot))
        })
    }
}
```

## Algorithm

### Overview

DFS flattening into a single flat vec. No side tables. Four methods:

- **`flatten_action`** — allocate a slot, then `flatten_action_at` into it. Returns ActionId.
- **`flatten_action_at`** — write an action's root entry into a given slot. The single match over all Action variants. For Pipe/Parallel/Branch, also `alloc_many` for child slots and `fill_child_slots`.
- **`fill_child_slot`** — put an action into a child slot. Single-entry children are inlined via `flatten_action_at`. Multi-entry children (Pipe/Parallel/Branch) are flattened elsewhere via `flatten_action` and a ChildRef is written into the slot.
- **`fill_child_slots`** — loop: call `fill_child_slot` for each child.

Handlers are interned via `IndexSet<HandlerKind>`: `insert_full` returns `(index, was_new)`, giving O(1) dedup with stable insertion-order indices. The `IndexSet` lives on `UnresolvedFlatConfig` during building; `resolve()` converts it to a `Vec<HandlerKind>` for the final `FlatConfig`.

KindDiscriminator is already an interned StringKey (Copy, u32-sized). No second interning needed — BranchKey entries use it directly.

### Two-pass resolution

**Pass 1**: DFS-flatten each tree. Step(Named(name)) stores `StepTarget::Named(name)`. Step(Root) resolves immediately to `StepTarget::Resolved(workflow_root)`.

**Pass 2**: Walk the vec via `try_map_target`. `StepTarget::Named(name)` resolves to `step_roots[name]`. `StepTarget::Resolved(id)` passes through. ChildRef and BranchKey entries pass through unchanged.

### Pass 1: UnresolvedFlatConfig

`UnresolvedFlatConfig` is the builder — the unresolved version of `FlatConfig`. It holds the entry array (with `Option` placeholders for pre-allocated slots) and the handler interning state. It has no `workflow_root` field; that's determined by the caller and threaded through as a parameter.

`workflow_root: Option<ActionId>` is `Some` when flattening the workflow tree (Step(Root) resolves to it) and `None` when flattening step bodies (Step(Root) in a step body is an error — see CONFIG_VALIDATION.md rule #5).

```rust
struct UnresolvedFlatConfig {
    entries: Vec<Option<FlatEntry<StepTarget>>>,
    handlers: IndexSet<HandlerKind>,
}

impl UnresolvedFlatConfig {
    fn alloc(&mut self) -> ActionId {
        let id = ActionId(self.entries.len() as u32);
        self.entries.push(None);
        id
    }

    /// Pre-allocate `count` contiguous None slots.
    fn alloc_many(&mut self, count: Count) {
        self.entries.extend(std::iter::repeat_n(None, count.0 as usize));
    }

    fn intern_handler(&mut self, handler: HandlerKind) -> HandlerId {
        let (index, _) = self.handlers.insert_full(handler);
        HandlerId(index as u32)
    }

    /// Allocate a slot, flatten an action into it, return its ActionId.
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
    /// over all Action variants — no duplication.
    ///
    /// For Pipe/Parallel/Branch, child slots are alloc_many'd immediately
    /// after the slot. This means the slot must be at the end of the vec
    /// for multi-entry actions (guaranteed when called from flatten_action).
    fn flatten_action_at(
        &mut self,
        action: Action,
        action_id: ActionId,
        workflow_root: Option<ActionId>,
    ) -> Result<(), FlattenError> {
        let entry = match action {
            Action::Invoke { handler } => {
                let handler_id = self.intern_handler(handler);
                FlatAction::Invoke { handler: handler_id }
            }

            Action::Pipe { actions } => {
                let count = Count(actions.len() as u32);
                self.alloc_many(count);
                self.fill_child_slots(actions, action_id + 1, workflow_root)?;
                FlatAction::Pipe { count }
            }

            Action::Parallel { actions } => {
                let count = Count(actions.len() as u32);
                self.alloc_many(count);
                self.fill_child_slots(actions, action_id + 1, workflow_root)?;
                FlatAction::Parallel { count }
            }

            Action::Branch { cases } => {
                let count = Count(cases.len() as u32);
                let mut cases: Vec<_> = cases.into_iter().collect();
                cases.sort_by_key(|(key, _)| *key);
                self.alloc_many(Count(2 * count.0));
                for (i, (key, child)) in cases.into_iter().enumerate() {
                    let key_slot = action_id + 1 + 2 * i as u32;
                    self.entries[key_slot.0 as usize] =
                        Some(FlatEntry::BranchKey { key });
                    self.fill_child_slot(child, key_slot + 1, workflow_root)?;
                }
                FlatAction::Branch { count }
            }

            Action::ForEach { body } => {
                let body_id = self.flatten_action(*body, workflow_root)?;
                FlatAction::ForEach { body: body_id }
            }

            Action::Loop { body } => {
                let body_id = self.flatten_action(*body, workflow_root)?;
                FlatAction::Loop { body: body_id }
            }

            Action::Attempt { action } => {
                let child_id = self.flatten_action(*action, workflow_root)?;
                FlatAction::Attempt { child: child_id }
            }

            Action::Step(StepRef::Named(name)) => {
                FlatAction::Step { target: StepTarget::Named(name) }
            }

            Action::Step(StepRef::Root) => {
                let root = workflow_root.ok_or(FlattenError::StepRootInStepBody)?;
                FlatAction::Step { target: StepTarget::Resolved(root) }
            }
        };
        self.entries[action_id.0 as usize] = Some(FlatEntry::Action(entry));
        Ok(())
    }

    /// Fill a child slot with an action. Single-entry actions are inlined
    /// directly into the slot (the FlatConfigEntryId becomes an ActionId).
    /// Multi-entry actions (Pipe/Parallel/Branch) are flattened elsewhere
    /// via flatten_action, and a ChildRef is written into the slot.
    fn fill_child_slot(
        &mut self,
        action: Action,
        slot: FlatConfigEntryId,
        workflow_root: Option<ActionId>,
    ) -> Result<(), FlattenError> {
        match action {
            Action::Pipe { .. } | Action::Parallel { .. } | Action::Branch { .. } => {
                let action_id = self.flatten_action(action, workflow_root)?;
                self.entries[slot.0 as usize] =
                    Some(FlatEntry::ChildRef { action: action_id });
            }
            single_entry => {
                // Inline: this child slot IS the action. Convert to ActionId.
                self.flatten_action_at(single_entry, ActionId(slot.0), workflow_root)?;
            }
        }
        Ok(())
    }

    /// Fill contiguous child slots from a Vec<Action>.
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

    /// Resolve step names and produce the final FlatConfig.
    fn resolve(
        self,
        workflow_root: ActionId,
        step_roots: &HashMap<StepName, ActionId>,
    ) -> Result<FlatConfig, FlattenError> {
        let entries = self.entries
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
            handlers: self.handlers.into_iter().collect(),
            workflow_root,
        })
    }
}
```

Branch cases are sorted by key for deterministic ActionId assignment.

### Top-level flatten

All errors from pass 1 and pass 2 are returned to the caller. The caller decides whether to panic or display them. In practice, CONFIG_VALIDATION.md catches these issues before flattening, so errors here indicate a bug.

```rust
fn flatten(config: Config) -> Result<FlatConfig, FlattenError> {
    let mut unresolved_flat_config = UnresolvedFlatConfig::new();

    // The workflow root will be at the next alloc position.
    let workflow_root = ActionId(unresolved_flat_config.entries.len() as u32);
    unresolved_flat_config.flatten_action(config.workflow, Some(workflow_root))?;

    let mut step_roots = HashMap::new();
    for (name, step_action) in config.steps {
        let step_root = unresolved_flat_config.flatten_action(step_action, None)?;
        step_roots.insert(name, step_root);
    }

    unresolved_flat_config.resolve(workflow_root, &step_roots)
}
```

## Unit tests

Tests follow the pipeline: build `Config` -> flatten -> assert `FlatConfig`.

### Basic structure

```rust
/// Single invoke: one entry, root = 0.
fn flatten_single_invoke()
// entries: [Invoke { handler: 0 }]

/// Pipe: all single-entry children inlined.
fn flatten_pipe()
// entries: [Pipe{3}, Invoke(0), Invoke(1), Invoke(2)]

/// Parallel: same layout as Pipe but with Parallel variant.
fn flatten_parallel()

/// ForEach: explicit body ActionId.
fn flatten_foreach()
// entries: [ForEach { body: 1 }, Invoke(0)]

/// Branch: BranchKey + inlined child pairs.
fn flatten_branch()
// entries: [Branch{2}, BranchKey("Err"), Invoke(0), BranchKey("Ok"), Invoke(1)]

/// Loop: explicit body ActionId.
fn flatten_loop()
// entries: [Loop { body: 1 }, Invoke(0)]

/// Attempt: explicit child ActionId.
fn flatten_attempt()
// entries: [Attempt { child: 1 }, Invoke(0)]
```

### Nesting

```rust
/// Nested pipe: inner pipe uses ChildRef, leaves inlined.
fn flatten_nested_pipe()

/// Single-child chain: Loop > Attempt > ForEach with explicit ActionIds.
fn flatten_single_child_chain()
// entries: [Loop{body:1}, Attempt{child:2}, ForEach{body:3}, Invoke(0)]

/// Deep nesting: Attempt > Loop > Pipe.
fn flatten_deep_nesting()

/// Pipe inside Parallel inside Loop.
fn flatten_nested_combinators()

/// Branch cases containing compound subtrees.
fn flatten_branch_with_subtrees()

/// Parallel containing Parallels (ChildRef for each).
fn flatten_parallel_of_parallels()

/// Pipe with Loop child: Loop inlined, body elsewhere.
fn flatten_pipe_with_loop_child()
```

### Step resolution

```rust
/// Step(Root) resolved immediately to workflow root ActionId.
fn flatten_step_root()

/// Named step resolved in pass 2.
fn flatten_step_named()

/// Mutual recursion: A -> B -> A, no infinite loop.
fn flatten_mutual_recursion()

/// Self-recursion: step body contains Step(Root).
fn flatten_self_recursion()

/// Chain of steps: A -> B -> C -> Invoke.
fn flatten_chain_of_steps()
```

### JSON round-trips

```rust
/// Parse JSON -> deserialize Config -> flatten -> assert.
fn flatten_from_json_simple_pipe()

/// JSON workflow with named steps.
fn flatten_from_json_with_steps()

/// JSON with Branch.
fn flatten_from_json_branch()

/// Complex workflow exercising all entry types.
fn flatten_from_json_kitchen_sink()
```

### Edge cases

```rust
/// Single-child pipe: child inlined.
fn flatten_single_child_pipe()

/// Single-case branch: BranchKey + inlined child.
fn flatten_single_case_branch()

/// Unknown step name panics.
#[should_panic(expected = "unknown step")]
fn flatten_unknown_step_panics()

/// Deterministic: flatten twice, assert identical.
fn flatten_deterministic()

/// Handler interning: identical handlers share the same HandlerId.
fn flatten_handler_interning()

/// Static assert: FlatEntry<ActionId> fits in 8 bytes.
fn flat_entry_size()
```

### Structural invariants

```rust
/// ChildRef pointing to another ChildRef panics.
#[should_panic]
fn childref_to_childref_panics()

/// ChildRef pointing to a BranchKey panics.
#[should_panic]
fn childref_to_branchkey_panics()

/// action() panics when given an ActionId that points to a ChildRef.
#[should_panic]
fn action_rejects_childref()

/// action() panics when given an ActionId that points to a BranchKey.
#[should_panic]
fn action_rejects_branchkey()

/// BranchKey in a Pipe/Parallel child slot panics.
#[should_panic]
fn branchkey_in_pipe_slot_panics()
```
