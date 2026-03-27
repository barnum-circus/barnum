# Flattening

The nested `Config` (tree of `Action` nodes with step references by name) is flattened into a `FlatConfig`: a linear array of entries where all cross-references are `ActionId` indices. No heap allocation. No side tables — all data is inline in the entries vec.

## Types

**Note:** `ActionId`, `EntryId`, `HandlerId`, and `Count` are `u32` newtypes via a `u32_newtype!` macro (modeled after isograph's `u64_newtypes` crate). To be created when implementation starts.

**`ActionId` vs `EntryId`:** Both are indices into `FlatConfig::entries`. `ActionId` is guaranteed to point to an action entry (Invoke, Pipe, Parallel, ForEach, Branch, Loop, Attempt, Step). `EntryId` is a raw position that might also be a ChildRef or BranchKey — used for child slot positions computed by arithmetic on a parent's position. `resolve_child_slot(EntryId) -> ActionId` resolves the indirection: if the slot holds a ChildRef, follow the pointer; if it holds an inlined action, return the slot as an ActionId.

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
    UninitializedEntry { index: EntryId },
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
    /// (ChildRef, BranchKey) indexed by EntryId. Use `action()` to look up
    /// an ActionId; use `resolve_child_slot()` to resolve an EntryId.
    entries: Vec<FlatEntry<ActionId>>,

    /// Handler pool. HandlerIds are indices into this vec.
    /// Handlers are interned: identical handlers share a HandlerId.
    handlers: Vec<HandlerKind>,

    /// Entry point for execution.
    workflow_root: ActionId,
}
```

### Accessors

The interpreter works with `ActionId` and `FlatAction`. Child slots (positions after Pipe/Parallel/Branch) are `EntryId`s — resolve them via `resolve_child_slot` to get an `ActionId`, then call `action()` to get the `FlatAction`.

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

    /// Resolve a child slot (EntryId) to an ActionId.
    /// - Inlined action: the slot position is the ActionId.
    /// - ChildRef: follow the pointer; validate target is an action via `action()`.
    /// - BranchKey in a child slot is a bug.
    fn resolve_child_slot(&self, slot: EntryId) -> ActionId {
        match self.entries[slot.0 as usize] {
            FlatEntry::Action(_) => ActionId(slot.0),
            FlatEntry::ChildRef { action } => {
                // Validate the target is actually an action (panics if not).
                self.action(action);
                action
            }
            FlatEntry::BranchKey { .. } => panic!("unexpected BranchKey in child slot"),
        }
    }

    /// Returns the child ActionIds for a Pipe or Parallel.
    fn children(&self, id: ActionId) -> impl Iterator<Item = ActionId> + '_ {
        let count = match self.action(id) {
            FlatAction::Pipe { count } | FlatAction::Parallel { count } => count.0,
            other => panic!("expected Pipe or Parallel, got {other:?}"),
        };
        let start = id.0 + 1;
        (0..count).map(move |i| self.resolve_child_slot(EntryId(start + i)))
    }

    /// Returns (key, action) pairs for a Branch.
    fn branch_cases(&self, id: ActionId) -> impl Iterator<Item = (KindDiscriminator, ActionId)> + '_ {
        let count = match self.action(id) {
            FlatAction::Branch { count } => count.0,
            other => panic!("expected Branch, got {other:?}"),
        };
        let start = id.0 + 1;
        (0..count).map(move |i| {
            let key_slot = EntryId(start + 2 * i);
            let key = match self.entries[key_slot.0 as usize] {
                FlatEntry::BranchKey { key } => key,
                other => panic!("expected BranchKey at {key_slot:?}, got {other:?}"),
            };
            let child_slot = EntryId(start + 2 * i + 1);
            let action = self.resolve_child_slot(child_slot);
            (key, action)
        })
    }
}
```

## Algorithm

### Overview

DFS flattening into a single flat vec. No side tables.

**Pipe/Parallel:** Allocate the instruction entry, then reserve `count` child slots. For each child: if the child is a multi-entry node (Pipe/Parallel/Branch), flatten it elsewhere and write a ChildRef into the slot. Otherwise, write the child entry directly into the slot via `flatten_into`.

**Branch:** Allocate the instruction entry, then reserve `2 * count` slots (alternating BranchKey/child). Same inlining rules apply to each child slot.

**ForEach/Loop/Attempt:** Allocate the entry, flatten the body/child (which allocates it next in DFS order), and store the body's ActionId in the entry. No implicit "self+1" convention — bodies are always referenced by explicit ActionId.

**Step(Named(name))** stores the name, resolved in pass 2. **Step(Root)** resolves immediately — the workflow root ActionId is known the moment we start flattening the workflow (it's the next `alloc()`).

Handlers are interned via `IndexSet<HandlerKind>`: `insert_full` returns `(index, was_new)`, giving O(1) dedup with stable insertion-order indices. The `IndexSet` lives on `UnresolvedFlatConfig` during building; `resolve()` converts it to a `Vec<HandlerKind>` for the final `FlatConfig`.

KindDiscriminator is already an interned StringKey (Copy, u32-sized). No second interning needed — BranchKey entries use it directly.

### Two-pass resolution

**Pass 1**: DFS-flatten each tree. Step(Named(name)) stores `StepTarget::Named(name)`. Step(Root) resolves immediately to `StepTarget::Resolved(workflow_root)`.

**Pass 2**: Walk the vec via `map_target`. `StepTarget::Named(name)` resolves to `step_roots[name]`. `StepTarget::Resolved(id)` passes through. ChildRef and BranchKey entries pass through unchanged.

### Pass 1: UnresolvedFlatConfig

`UnresolvedFlatConfig` is the builder — the unresolved version of `FlatConfig`. It holds the entry array (with `Option` placeholders for pre-allocated slots) and the handler interning state. It has no `workflow_root` field; that's determined by the caller and threaded through as a parameter.

`workflow_root: Option<ActionId>` is `Some` when flattening the workflow tree (Step(Root) resolves to it) and `None` when flattening step bodies (Step(Root) in a step body panics — see CONFIG_VALIDATION.md rule #5).

```rust
struct UnresolvedFlatConfig {
    entries: Vec<Option<FlatEntry<StepTarget>>>,
    handlers: IndexSet<HandlerKind>,
}

impl UnresolvedFlatConfig {
    fn alloc(&mut self) -> EntryId {
        let id = EntryId(self.entries.len() as u32);
        self.entries.push(None);
        id
    }

    /// Pre-allocate `count` contiguous slots. Returns the EntryId of the first slot.
    fn alloc_many(&mut self, count: Count) -> EntryId {
        let start = EntryId(self.entries.len() as u32);
        self.entries.extend(std::iter::repeat_n(None, count.0 as usize));
        start
    }

    fn intern_handler(&mut self, handler: HandlerKind) -> HandlerId {
        let (index, _) = self.handlers.insert_full(handler);
        HandlerId(index as u32)
    }

    fn is_multi_entry(node: &Action) -> bool {
        matches!(node, Action::Pipe { .. } | Action::Parallel { .. } | Action::Branch { .. })
    }

    /// Flatten a single-entry node directly into a pre-allocated child slot.
    /// Takes ownership of the node. Multi-entry nodes cannot be inlined — use ChildRef.
    fn flatten_into(
        &mut self,
        node: Action,
        slot: EntryId,
        workflow_root: Option<ActionId>,
    ) -> Result<(), FlattenError> {
        let entry = match node {
            Action::Invoke { handler } => {
                let handler_id = self.intern_handler(handler);
                FlatEntry::Action(FlatAction::Invoke { handler: handler_id })
            }
            Action::ForEach { body } => {
                let body_id = self.flatten_node(*body, workflow_root)?;
                FlatEntry::Action(FlatAction::ForEach { body: body_id })
            }
            Action::Loop { body } => {
                let body_id = self.flatten_node(*body, workflow_root)?;
                FlatEntry::Action(FlatAction::Loop { body: body_id })
            }
            Action::Attempt { action } => {
                let child_id = self.flatten_node(*action, workflow_root)?;
                FlatEntry::Action(FlatAction::Attempt { child: child_id })
            }
            Action::Step(StepRef::Named(name)) => {
                FlatEntry::Action(FlatAction::Step {
                    target: StepTarget::Named(name),
                })
            }
            Action::Step(StepRef::Root) => {
                let root = workflow_root.ok_or(FlattenError::StepRootInStepBody)?;
                FlatEntry::Action(FlatAction::Step {
                    target: StepTarget::Resolved(root),
                })
            }
            _ => panic!("multi-entry nodes cannot be inlined"),
        };
        self.entries[slot.0 as usize] = Some(entry);
        Ok(())
    }

    /// Fill child slots for a Pipe or Parallel. Takes ownership of the action vec.
    fn flatten_children(
        &mut self,
        actions: Vec<Action>,
        ref_start: EntryId,
        workflow_root: Option<ActionId>,
    ) -> Result<(), FlattenError> {
        for (i, child) in actions.into_iter().enumerate() {
            let slot = EntryId(ref_start.0 + i as u32);
            if Self::is_multi_entry(&child) {
                let child_root = self.flatten_node(child, workflow_root)?;
                self.entries[slot.0 as usize] =
                    Some(FlatEntry::ChildRef { action: child_root });
            } else {
                self.flatten_into(child, slot, workflow_root)?;
            }
        }
        Ok(())
    }

    /// Flatten an action node, taking ownership. Returns the root ActionId.
    fn flatten_node(
        &mut self,
        node: Action,
        workflow_root: Option<ActionId>,
    ) -> Result<ActionId, FlattenError> {
        match node {
            Action::Invoke { handler } => {
                let id = self.alloc();
                let handler_id = self.intern_handler(handler);
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Action(FlatAction::Invoke { handler: handler_id }));
                Ok(ActionId(id.0))
            }

            Action::Pipe { actions } => {
                let id = self.alloc();
                let count = Count(actions.len() as u32);
                let ref_start = self.alloc_many(count);
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Action(FlatAction::Pipe { count }));
                self.flatten_children(actions, ref_start, workflow_root)?;
                Ok(ActionId(id.0))
            }

            Action::Parallel { actions } => {
                let id = self.alloc();
                let count = Count(actions.len() as u32);
                let ref_start = self.alloc_many(count);
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Action(FlatAction::Parallel { count }));
                self.flatten_children(actions, ref_start, workflow_root)?;
                Ok(ActionId(id.0))
            }

            Action::ForEach { body } => {
                let id = self.alloc();
                let body_id = self.flatten_node(*body, workflow_root)?;
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Action(FlatAction::ForEach { body: body_id }));
                Ok(ActionId(id.0))
            }

            Action::Branch { mut cases } => {
                let id = self.alloc();
                let count = Count(cases.len() as u32);
                let mut sorted_keys: Vec<_> = cases.keys().cloned().collect();
                sorted_keys.sort();
                let ref_start = self.alloc_many(Count(2 * count.0));
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Action(FlatAction::Branch { count }));
                for (i, key) in sorted_keys.into_iter().enumerate() {
                    let key_slot = EntryId(ref_start.0 + 2 * i as u32);
                    self.entries[key_slot.0 as usize] =
                        Some(FlatEntry::BranchKey { key });
                    let child = cases.remove(&key).unwrap();
                    let child_slot = EntryId(ref_start.0 + 2 * i as u32 + 1);
                    if Self::is_multi_entry(&child) {
                        let case_root = self.flatten_node(child, workflow_root)?;
                        self.entries[child_slot.0 as usize] =
                            Some(FlatEntry::ChildRef { action: case_root });
                    } else {
                        self.flatten_into(child, child_slot, workflow_root)?;
                    }
                }
                Ok(ActionId(id.0))
            }

            Action::Loop { body } => {
                let id = self.alloc();
                let body_id = self.flatten_node(*body, workflow_root)?;
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Action(FlatAction::Loop { body: body_id }));
                Ok(ActionId(id.0))
            }

            Action::Attempt { action } => {
                let id = self.alloc();
                let child_id = self.flatten_node(*action, workflow_root)?;
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Action(FlatAction::Attempt { child: child_id }));
                Ok(ActionId(id.0))
            }

            Action::Step(StepRef::Named(name)) => {
                let id = self.alloc();
                self.entries[id.0 as usize] = Some(FlatEntry::Action(FlatAction::Step {
                    target: StepTarget::Named(name),
                }));
                Ok(ActionId(id.0))
            }

            Action::Step(StepRef::Root) => {
                let root = workflow_root.ok_or(FlattenError::StepRootInStepBody)?;
                let id = self.alloc();
                self.entries[id.0 as usize] = Some(FlatEntry::Action(FlatAction::Step {
                    target: StepTarget::Resolved(root),
                }));
                Ok(ActionId(id.0))
            }
        }
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
                    index: EntryId(i as u32),
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
    unresolved_flat_config.flatten_node(config.workflow, Some(workflow_root))?;

    let mut step_roots = HashMap::new();
    for (name, step_action) in config.steps {
        let step_root = unresolved_flat_config.flatten_node(step_action, None)?;
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
