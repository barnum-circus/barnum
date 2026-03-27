# Flattening

The nested `Config` (tree of `Action` nodes with step references by name) is flattened into a `FlatConfig`: a linear array of entries where all cross-references are `ActionId` indices. No heap allocation. No side tables — all data is inline in the entries vec.

## Types

**Note:** `ActionId`, `HandlerId`, and `Count` are `u32` newtypes via a `u32_newtype!` macro (modeled after isograph's `u64_newtypes` crate). To be created when implementation starts.

```rust
/// An entry in the flat action table.
///
/// Multi-child instructions (Pipe, Parallel) store a `count` and are followed
/// by `count` child slots. Each child slot is either:
/// - A ChildRef (if the child is a multi-entry node: Pipe/Parallel/Branch)
/// - The child entry directly (if the child is single-entry: Invoke/Step/ForEach/Loop/Attempt)
///
/// Branch stores a `count` (number of cases) and is followed by `2 * count`
/// entries: `count` alternating pairs of (BranchKey, child slot). Same inlining
/// rules apply to the child slot in each pair.
///
/// ChildRef and BranchKey are inline data, not instructions. They are only
/// valid in child slots after a Pipe, Parallel, or Branch entry.
///
/// Generic over `T`: the Step target type. During pass 1, `T = StepTarget`
/// (may contain unresolved step names). After pass 2, `T = ActionId`
/// (fully resolved). The generic applies only to Step.
///
/// `FlatEntry<ActionId>` is Copy (all fields are u32 newtypes).
/// `FlatEntry<StepTarget>` is Clone but not Copy (StepTarget::Named holds a String).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FlatEntry<T> {
    /// Leaf: invoke a handler. `handler` indexes `FlatConfig::handlers`.
    Invoke { handler: HandlerId },

    /// Sequential composition.
    /// Followed by `count` child slots (ChildRef or inlined entry).
    Pipe { count: Count },

    /// Fan-out: same input to all children, collect results as array.
    /// Followed by `count` child slots (ChildRef or inlined entry).
    Parallel { count: Count },

    /// Map over array input.
    ForEach { body: ActionId },

    /// Case analysis on value["kind"].
    /// Followed by `2 * count` entries: `count` pairs of (BranchKey, child slot).
    /// Each BranchKey holds the discriminator; the child slot holds the case's
    /// root entry (ChildRef or inlined).
    Branch { count: Count },

    /// Loop: runs body, inspects result variant to break or continue.
    Loop { body: ActionId },

    /// Error materialization.
    Attempt { child: ActionId },

    /// Redirect to another action (step reference or self-recursion).
    Step { target: T },

    // -- Inline data (not instructions) --

    /// Child pointer for multi-entry children (Pipe/Parallel/Branch).
    /// Points to the root ActionId of a child subtree.
    /// Only appears in child slots when the child is itself a multi-entry node.
    ChildRef { action: ActionId },

    /// Branch case key. Always immediately followed by a child slot.
    /// Only valid after a Branch entry.
    BranchKey { key: KindDiscriminator },
}
```

Every variant has at most one `u32` field. Discriminant + payload fits in 8 bytes. Enforce with `static_assert!(size_of::<FlatEntry<ActionId>>() <= 8)`.

**Future optimization:** Branch cases currently use two entries each (BranchKey + child slot). If KindDiscriminator is narrowed to `u16`, a single `BranchCase { key: u16, action: ActionId }` variant (6 bytes payload, fits in 8 with discriminant) could halve branch overhead.

```rust
impl<T> FlatEntry<T> {
    fn map_target<U>(self, f: impl FnOnce(T) -> U) -> FlatEntry<U> {
        match self {
            FlatEntry::Step { target } => FlatEntry::Step { target: f(target) },
            FlatEntry::Invoke { handler } => FlatEntry::Invoke { handler },
            FlatEntry::Pipe { count } => FlatEntry::Pipe { count },
            FlatEntry::Parallel { count } => FlatEntry::Parallel { count },
            FlatEntry::ForEach { body } => FlatEntry::ForEach { body },
            FlatEntry::Branch { count } => FlatEntry::Branch { count },
            FlatEntry::Loop { body } => FlatEntry::Loop { body },
            FlatEntry::Attempt { child } => FlatEntry::Attempt { child },
            FlatEntry::ChildRef { action } => FlatEntry::ChildRef { action },
            FlatEntry::BranchKey { key } => FlatEntry::BranchKey { key },
        }
    }
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
    /// The entry array. ActionIds are indices into this vec.
    /// Contains instructions, inline child slots (ChildRef or inlined entries),
    /// and BranchKey entries.
    entries: Vec<FlatEntry<ActionId>>,

    /// Handler pool. HandlerIds are indices into this vec.
    /// Handlers are interned: identical handlers share a HandlerId.
    handlers: Vec<HandlerKind>,

    /// Entry point for execution.
    workflow_root: ActionId,
}
```

### Accessors

The interpreter accesses entries by `ActionId`. Child slots may contain either a ChildRef (pointer to a multi-entry child) or an inlined entry (the child itself). These accessors encapsulate that.

```rust
impl FlatConfig {
    fn entry(&self, id: ActionId) -> FlatEntry<ActionId> {
        self.entries[id.0 as usize]
    }

    fn handler(&self, id: HandlerId) -> &HandlerKind {
        &self.handlers[id.0 as usize]
    }

    /// Resolve a child slot: if ChildRef, follow the pointer;
    /// otherwise the slot index itself is the child's ActionId.
    fn resolve_child_slot(&self, slot: usize) -> ActionId {
        match self.entries[slot] {
            FlatEntry::ChildRef { action } => action,
            _ => ActionId(slot as u32),
        }
    }

    /// Returns the child ActionIds for a Pipe or Parallel.
    fn children(&self, id: ActionId) -> impl Iterator<Item = ActionId> + '_ {
        let count = match self.entry(id) {
            FlatEntry::Pipe { count } | FlatEntry::Parallel { count } => count.0 as usize,
            other => panic!("expected Pipe or Parallel, got {other:?}"),
        };
        let start = id.0 as usize + 1;
        (0..count).map(move |i| self.resolve_child_slot(start + i))
    }

    /// Returns (key, action) pairs for a Branch.
    fn branch_cases(&self, id: ActionId) -> impl Iterator<Item = (KindDiscriminator, ActionId)> + '_ {
        let count = match self.entry(id) {
            FlatEntry::Branch { count } => count.0 as usize,
            other => panic!("expected Branch, got {other:?}"),
        };
        let start = id.0 as usize + 1;
        (0..count).map(move |i| {
            let key = match self.entries[start + 2 * i] {
                FlatEntry::BranchKey { key } => key,
                other => panic!("expected BranchKey, got {other:?}"),
            };
            let action = self.resolve_child_slot(start + 2 * i + 1);
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

Handlers are interned via `IndexSet<HandlerKind>`: identical handlers share a `HandlerId`. `IndexSet::insert_full` returns the index (existing or new), giving O(1) dedup with stable insertion-order indices.

KindDiscriminator is already an interned StringKey (Copy, u32-sized). No second interning needed — BranchKey entries use it directly.

### Two-pass resolution

**Pass 1**: DFS-flatten each tree. Step(Named(name)) stores `StepTarget::Named(name)`. Step(Root) resolves immediately to `StepTarget::Resolved(workflow_root)`.

**Pass 2**: Walk the vec via `map_target`. `StepTarget::Named(name)` resolves to `step_roots[name]`. `StepTarget::Resolved(id)` passes through. ChildRef and BranchKey entries pass through unchanged.

### Pass 1: Flattener

```rust
struct Flattener {
    entries: Vec<Option<FlatEntry<StepTarget>>>,
    handlers: IndexSet<HandlerKind>,
    /// Set before flattening the workflow. Step(Root) resolves to this immediately.
    workflow_root: ActionId,
}

impl Flattener {
    fn alloc(&mut self) -> ActionId {
        let id = ActionId(self.entries.len() as u32);
        self.entries.push(None);
        id
    }

    fn intern_handler(&mut self, handler: HandlerKind) -> HandlerId {
        let (index, _) = self.handlers.insert_full(handler);
        HandlerId(index as u32)
    }

    fn is_multi_entry(node: &Action) -> bool {
        matches!(node, Action::Pipe { .. } | Action::Parallel { .. } | Action::Branch { .. })
    }

    /// Flatten a single-entry node into a pre-allocated child slot.
    /// Multi-entry nodes (Pipe/Parallel/Branch) cannot be inlined — use ChildRef.
    fn flatten_into(&mut self, node: &Action, slot: usize) {
        match node {
            Action::Invoke { handler } => {
                let handler_id = self.intern_handler(handler.clone());
                self.entries[slot] = Some(FlatEntry::Invoke { handler: handler_id });
            }
            Action::ForEach { body } => {
                let body_id = self.flatten_node(body);
                self.entries[slot] = Some(FlatEntry::ForEach { body: body_id });
            }
            Action::Loop { body } => {
                let body_id = self.flatten_node(body);
                self.entries[slot] = Some(FlatEntry::Loop { body: body_id });
            }
            Action::Attempt { action } => {
                let child_id = self.flatten_node(action);
                self.entries[slot] = Some(FlatEntry::Attempt { child: child_id });
            }
            Action::Step(StepRef::Named(name)) => {
                self.entries[slot] = Some(FlatEntry::Step {
                    target: StepTarget::Named(name.clone()),
                });
            }
            Action::Step(StepRef::Root) => {
                self.entries[slot] = Some(FlatEntry::Step {
                    target: StepTarget::Resolved(self.workflow_root),
                });
            }
            _ => panic!("multi-entry nodes cannot be inlined"),
        }
    }

    /// Fill child slots for a Pipe or Parallel.
    fn flatten_children(&mut self, actions: &[Action], ref_start: usize) {
        for (i, child) in actions.iter().enumerate() {
            let slot = ref_start + i;
            if Self::is_multi_entry(child) {
                let child_root = self.flatten_node(child);
                self.entries[slot] =
                    Some(FlatEntry::ChildRef { action: child_root });
            } else {
                self.flatten_into(child, slot);
            }
        }
    }

    fn flatten_node(&mut self, node: &Action) -> ActionId {
        match node {
            Action::Invoke { handler } => {
                let id = self.alloc();
                let handler_id = self.intern_handler(handler.clone());
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Invoke { handler: handler_id });
                id
            }

            Action::Pipe { actions } => {
                let id = self.alloc();
                let count = Count(actions.len() as u32);
                let ref_start = self.entries.len();
                for _ in 0..count.0 {
                    self.alloc();
                }
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Pipe { count });
                self.flatten_children(actions, ref_start);
                id
            }

            Action::Parallel { actions } => {
                let id = self.alloc();
                let count = Count(actions.len() as u32);
                let ref_start = self.entries.len();
                for _ in 0..count.0 {
                    self.alloc();
                }
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Parallel { count });
                self.flatten_children(actions, ref_start);
                id
            }

            Action::ForEach { body } => {
                let id = self.alloc();
                let body_id = self.flatten_node(body);
                self.entries[id.0 as usize] =
                    Some(FlatEntry::ForEach { body: body_id });
                id
            }

            Action::Branch { cases } => {
                let id = self.alloc();
                let count = Count(cases.len() as u32);
                let mut sorted_keys: Vec<_> = cases.keys().collect();
                sorted_keys.sort();
                let ref_start = self.entries.len();
                for _ in 0..(2 * count.0) {
                    self.alloc();
                }
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Branch { count });
                for (i, key) in sorted_keys.iter().enumerate() {
                    self.entries[ref_start + 2 * i] =
                        Some(FlatEntry::BranchKey { key: (*key).clone() });
                    let child = &cases[key];
                    let child_slot = ref_start + 2 * i + 1;
                    if Self::is_multi_entry(child) {
                        let case_root = self.flatten_node(child);
                        self.entries[child_slot] =
                            Some(FlatEntry::ChildRef { action: case_root });
                    } else {
                        self.flatten_into(child, child_slot);
                    }
                }
                id
            }

            Action::Loop { body } => {
                let id = self.alloc();
                let body_id = self.flatten_node(body);
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Loop { body: body_id });
                id
            }

            Action::Attempt { action } => {
                let id = self.alloc();
                let child_id = self.flatten_node(action);
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Attempt { child: child_id });
                id
            }

            Action::Step(StepRef::Named(name)) => {
                let id = self.alloc();
                self.entries[id.0 as usize] = Some(FlatEntry::Step {
                    target: StepTarget::Named(name.clone()),
                });
                id
            }

            Action::Step(StepRef::Root) => {
                let id = self.alloc();
                self.entries[id.0 as usize] = Some(FlatEntry::Step {
                    target: StepTarget::Resolved(self.workflow_root),
                });
                id
            }
        }
    }
}
```

Branch cases are sorted by key for deterministic ActionId assignment.

### Pass 2: resolve step names

```rust
fn resolve_targets(
    entries: Vec<Option<FlatEntry<StepTarget>>>,
    step_roots: &HashMap<StepName, ActionId>,
) -> Vec<FlatEntry<ActionId>> {
    entries
        .into_iter()
        .map(|slot| {
            slot.expect("uninitialized entry")
                .map_target(|target| match target {
                    StepTarget::Named(name) => *step_roots
                        .get(&name)
                        .unwrap_or_else(|| panic!("unknown step: {name}")),
                    StepTarget::Resolved(id) => id,
                })
        })
        .collect()
}
```

### Top-level flatten

```rust
fn flatten(config: &Config) -> FlatConfig {
    let mut f = Flattener::new();

    // Pass 1: DFS-flatten each tree.
    // Peek at the next ActionId — this will be the workflow root.
    f.workflow_root = ActionId(f.entries.len() as u32);
    let workflow_root = f.flatten_node(&config.workflow);

    let mut step_roots = HashMap::new();
    for (name, step_action) in &config.steps {
        let step_root = f.flatten_node(step_action);
        step_roots.insert(name.clone(), step_root);
    }

    // Pass 2: resolve step names to ActionIds.
    // Step(Root) was already resolved in pass 1.
    let entries = resolve_targets(f.entries, &step_roots);

    FlatConfig {
        entries,
        handlers: f.handlers.into_iter().collect(),
        workflow_root,
    }
}
```

## Examples

### Simple pipe

```
Input:  Pipe([Invoke(a), Invoke(b), Invoke(c)])

entries:
  0: Pipe { count: 3 }
  1: Invoke { handler: 0 }     -- a (inlined)
  2: Invoke { handler: 1 }     -- b (inlined)
  3: Invoke { handler: 2 }     -- c (inlined)

handlers: [a, b, c]
```

### Nested pipe

```
Input:  Pipe([Invoke(a), Pipe([Invoke(b), Invoke(c)]), Invoke(d)])

entries:
  0: Pipe { count: 3 }
  1: Invoke { handler: 0 }        -- a (inlined)
  2: ChildRef { action: 4 }       -- inner pipe (multi-entry, needs ChildRef)
  3: Invoke { handler: 3 }        -- d (inlined)
  4: Pipe { count: 2 }
  5: Invoke { handler: 1 }        -- b (inlined)
  6: Invoke { handler: 2 }        -- c (inlined)

handlers: [a, b, c, d]
```

### Single-child nodes

```
Input:  Loop(Attempt(ForEach(Invoke(process))))

entries:
  0: Loop { body: 1 }
  1: Attempt { child: 2 }
  2: ForEach { body: 3 }
  3: Invoke { handler: 0 }    -- process

(Bodies happen to be at self+1 due to DFS ordering,
but the interpreter uses the explicit ActionId, not the position.)
```

### Branch

```
Input:  Branch({ "Err": Invoke(handle_err), "Ok": Invoke(handle_ok) })

entries:
  0: Branch { count: 2 }
  1: BranchKey { key: "Err" }
  2: Invoke { handler: 0 }      -- handle_err (inlined)
  3: BranchKey { key: "Ok" }
  4: Invoke { handler: 1 }      -- handle_ok (inlined)
```

### Pipe with Loop child

```
Input:  Pipe([Invoke(a), Loop(Invoke(process))])

entries:
  0: Pipe { count: 2 }
  1: Invoke { handler: 0 }        -- a (inlined)
  2: Loop { body: 3 }             -- (inlined, body elsewhere)
  3: Invoke { handler: 1 }        -- process

(Loop is single-entry so it goes directly in the child slot.
Its body is flattened after the child slots.)
```

### Step resolution

```
Input:
  workflow: Pipe([Invoke(setup), Step(Named("Fix"))])
  steps: { Fix: Loop(Invoke(check)) }

Pass 1:
  entries:
    0: Pipe { count: 2 }
    1: Invoke { handler: 0 }              -- setup (inlined)
    2: Step { target: Named("Fix") }      -- (inlined)
    3: Loop { body: 4 }                   -- step "Fix" root
    4: Invoke { handler: 1 }              -- check

  step_roots: { "Fix" => 3 }

Pass 2:
  entries[2]: Step { target: 3 }
```

### Mutual recursion

```
Input:
  workflow: Step(Named("A"))
  steps: {
    A: Pipe([Invoke(do_a), Step(Named("B"))]),
    B: Pipe([Invoke(do_b), Step(Named("A"))]),
  }

Pass 1 flattens each tree independently. Step nodes store names, never follow
targets. Pass 2 resolves names to ActionIds. No infinite loop.
```

## Unit tests

Tests follow the pipeline: build `Config` -> flatten -> assert `FlatConfig`.

### Helpers

```rust
fn ts_handler(name: &str) -> HandlerKind { /* TypeScript handler with module="/m", func=name */ }
fn invoke(name: &str) -> Action { Action::Invoke(InvokeAction { handler: ts_handler(name) }) }
fn pipe(actions: Vec<Action>) -> Action { ... }
fn parallel(actions: Vec<Action>) -> Action { ... }
fn for_each(action: Action) -> Action { ... }
fn branch(cases: Vec<(&str, Action)>) -> Action { ... }
fn loop_(body: Action) -> Action { ... }
fn attempt(action: Action) -> Action { ... }
fn step_named(name: &str) -> Action { ... }
fn step_root() -> Action { ... }
fn config(workflow: Action) -> Config { Config { workflow, steps: HashMap::new() } }
fn config_with_steps(workflow: Action, steps: Vec<(&str, Action)>) -> Config { ... }
fn flatten(config: Config) -> FlatConfig { /* pass 1 + pass 2 */ }
```

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
