# Flattening

The nested `Config` (tree of `Action` nodes with step references by name) is flattened into a `FlatConfig`: a linear array of entries where all cross-references are `ActionId` indices. No heap allocation. No side tables — all data is inline in the entries vec.

## Types

**Note:** `ActionId`, `HandlerId`, and `Count` are `u32` newtypes via a `u32_newtype!` macro (modeled after isograph's `u64_newtypes` crate). To be created when implementation starts.

```rust
/// An entry in the flat action table.
///
/// Multi-child instructions (Pipe, Parallel) store a `count` and are followed
/// by `count` ChildRef entries inline in the entries vec.
///
/// Branch stores a `count` (number of cases) and is followed by `2 * count`
/// entries: `count` alternating pairs of (BranchKey, ChildRef).
///
/// Single-child instructions (ForEach, Loop, Attempt) have their child
/// at self+1 unconditionally. No field needed.
///
/// ChildRef and BranchKey are inline data, not instructions. They are only
/// valid immediately after a Pipe, Parallel, or Branch entry.
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
    /// Followed by `count` ChildRef entries pointing to children's roots.
    Pipe { count: Count },

    /// Fan-out: same input to all children, collect results as array.
    /// Followed by `count` ChildRef entries pointing to children's roots.
    Parallel { count: Count },

    /// Map over array input. Body at self+1.
    ForEach,

    /// Case analysis on value["kind"].
    /// Followed by `2 * count` entries: `count` pairs of (BranchKey, ChildRef).
    /// Each BranchKey holds the discriminator; the following ChildRef holds the
    /// ActionId of the matching case's root.
    Branch { count: Count },

    /// Fixed-point iteration. Body at self+1.
    Loop,

    /// Error materialization. Child at self+1.
    Attempt,

    /// Redirect to another action (step reference or self-recursion).
    Step { target: T },

    // -- Inline data (not instructions) --

    /// Child pointer. Points to the root ActionId of a child subtree.
    /// Only valid after Pipe, Parallel, or as part of a Branch case pair.
    ChildRef { action: ActionId },

    /// Branch case key. Always immediately followed by a ChildRef.
    /// Only valid after a Branch entry.
    BranchKey { key: KindDiscriminator },
}
```

Every variant has at most one `u32` field. Discriminant + payload fits in 8 bytes. Enforce with `static_assert!(size_of::<FlatEntry<ActionId>>() <= 8)`.

**Future optimization:** Branch cases currently use two entries each (BranchKey + ChildRef). If KindDiscriminator is narrowed to `u16`, a single `BranchCase { key: u16, action: ActionId }` variant (6 bytes payload, fits in 8 with discriminant) could halve branch overhead.

```rust
impl<T> FlatEntry<T> {
    fn map_target<U>(self, f: impl FnOnce(T) -> U) -> FlatEntry<U> {
        match self {
            FlatEntry::Step { target } => FlatEntry::Step { target: f(target) },
            FlatEntry::Invoke { handler } => FlatEntry::Invoke { handler },
            FlatEntry::Pipe { count } => FlatEntry::Pipe { count },
            FlatEntry::Parallel { count } => FlatEntry::Parallel { count },
            FlatEntry::ForEach => FlatEntry::ForEach,
            FlatEntry::Branch { count } => FlatEntry::Branch { count },
            FlatEntry::Loop => FlatEntry::Loop,
            FlatEntry::Attempt => FlatEntry::Attempt,
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
    /// Contains both instructions and inline data (ChildRef, BranchKey).
    entries: Vec<FlatEntry<ActionId>>,

    /// Handler pool. HandlerIds are indices into this vec.
    /// Handlers are interned: identical handlers share a HandlerId.
    handlers: Vec<HandlerKind>,

    /// Entry point for execution.
    workflow_root: ActionId,
}
```

Single-child instructions (ForEach, Loop, Attempt) have their child unconditionally at `self+1` — guaranteed by DFS ordering.

### Accessors

The interpreter accesses entries by `ActionId`, not by linear iteration. Inline data (ChildRef, BranchKey) is read relative to the parent instruction's position. These accessors encapsulate that.

```rust
impl FlatConfig {
    fn entry(&self, id: ActionId) -> FlatEntry<ActionId> {
        self.entries[id.0 as usize]
    }

    fn handler(&self, id: HandlerId) -> &HandlerKind {
        &self.handlers[id.0 as usize]
    }

    /// Returns the child ActionIds for a Pipe or Parallel.
    /// Reads the `count` inline ChildRef entries starting at `id + 1`.
    fn children(&self, id: ActionId) -> impl Iterator<Item = ActionId> + '_ {
        let count = match self.entry(id) {
            FlatEntry::Pipe { count } | FlatEntry::Parallel { count } => count.0 as usize,
            other => panic!("expected Pipe or Parallel, got {other:?}"),
        };
        let start = id.0 as usize + 1;
        self.entries[start..start + count].iter().map(|entry| match entry {
            FlatEntry::ChildRef { action } => *action,
            other => panic!("expected ChildRef, got {other:?}"),
        })
    }

    /// Returns (key, action) pairs for a Branch.
    /// Reads `2 * count` inline entries (alternating BranchKey / ChildRef)
    /// starting at `id + 1`.
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
            let action = match self.entries[start + 2 * i + 1] {
                FlatEntry::ChildRef { action } => action,
                other => panic!("expected ChildRef, got {other:?}"),
            };
            (key, action)
        })
    }
}
```

## Algorithm

### Overview

DFS flattening into a single flat vec. No side tables.

**Pipe/Parallel:** Allocate the instruction entry, then reserve `count` placeholder slots for ChildRef entries. DFS into each child, writing the root ActionId back into the corresponding ChildRef slot.

**Branch:** Allocate the instruction entry, then reserve `2 * count` placeholder slots for alternating BranchKey/ChildRef pairs. Iterate cases in sorted key order, writing the BranchKey and DFS-ing into each case body.

**Single-child nodes** (ForEach, Loop, Attempt): Allocate the instruction slot, then immediately DFS into the child. Since no other allocation happens between the instruction and the child, the child's root is guaranteed to be at self+1.

**Step(Named(name))** stores the name, resolved in pass 2. **Step(Root)** resolves immediately since the workflow root is always ActionId(0).

Handlers are interned via `IndexSet<HandlerKind>`: identical handlers share a `HandlerId`. `IndexSet::insert_full` returns the index (existing or new), giving O(1) dedup with stable insertion-order indices.

KindDiscriminator is already an interned StringKey (Copy, u32-sized). No second interning needed — BranchKey entries use it directly.

### Two-pass resolution

**Pass 1**: DFS-flatten each tree. Step(Named(name)) stores `StepTarget::Named(name)`. Step(Root) resolves immediately to `StepTarget::Resolved(ActionId(0))`.

**Pass 2**: Walk the vec, replacing `StepTarget::Named(name)` with `step_roots[name]` via `map_target`. ChildRef and BranchKey entries pass through unchanged.

### Pass 1: Flattener

```rust
struct Flattener {
    entries: Vec<Option<FlatEntry<StepTarget>>>,
    handlers: IndexSet<HandlerKind>,
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
                // Reserve ChildRef slots
                let ref_start = self.entries.len();
                for _ in 0..count.0 {
                    self.alloc();
                }
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Pipe { count });
                for (i, child) in actions.iter().enumerate() {
                    let child_root = self.flatten_node(child);
                    self.entries[ref_start + i] =
                        Some(FlatEntry::ChildRef { action: child_root });
                }
                id
            }

            Action::Parallel { actions } => {
                let id = self.alloc();
                let count = Count(actions.len() as u32);
                // Reserve ChildRef slots
                let ref_start = self.entries.len();
                for _ in 0..count.0 {
                    self.alloc();
                }
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Parallel { count });
                for (i, child) in actions.iter().enumerate() {
                    let child_root = self.flatten_node(child);
                    self.entries[ref_start + i] =
                        Some(FlatEntry::ChildRef { action: child_root });
                }
                id
            }

            Action::ForEach { body } => {
                let id = self.alloc();
                self.entries[id.0 as usize] = Some(FlatEntry::ForEach);
                self.flatten_node(body); // child root guaranteed at id+1
                id
            }

            Action::Branch { cases } => {
                let id = self.alloc();
                let count = Count(cases.len() as u32);
                let mut sorted_keys: Vec<_> = cases.keys().collect();
                sorted_keys.sort();
                // Reserve 2*count slots: alternating BranchKey / ChildRef
                let ref_start = self.entries.len();
                for _ in 0..(2 * count.0) {
                    self.alloc();
                }
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Branch { count });
                for (i, key) in sorted_keys.iter().enumerate() {
                    self.entries[ref_start + 2 * i] =
                        Some(FlatEntry::BranchKey { key: (*key).clone() });
                    let case_root = self.flatten_node(&cases[key]);
                    self.entries[ref_start + 2 * i + 1] =
                        Some(FlatEntry::ChildRef { action: case_root });
                }
                id
            }

            Action::Loop { body } => {
                let id = self.alloc();
                self.entries[id.0 as usize] = Some(FlatEntry::Loop);
                self.flatten_node(body); // child root guaranteed at id+1
                id
            }

            Action::Attempt { action } => {
                let id = self.alloc();
                self.entries[id.0 as usize] = Some(FlatEntry::Attempt);
                self.flatten_node(action); // child root guaranteed at id+1
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
                    target: StepTarget::Resolved(ActionId(0)),
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
    // Workflow root is always the first ActionId (0).
    let workflow_root = f.flatten_node(&config.workflow);
    debug_assert_eq!(workflow_root, ActionId(0));

    let mut step_roots = HashMap::new();
    for (name, step_action) in &config.steps {
        let step_root = f.flatten_node(step_action);
        step_roots.insert(name.clone(), step_root);
    }

    // Pass 2: resolve step names to ActionIds.
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
  1: ChildRef { action: 4 }    -- a
  2: ChildRef { action: 5 }    -- b
  3: ChildRef { action: 6 }    -- c
  4: Invoke { handler: 0 }     -- a
  5: Invoke { handler: 1 }     -- b
  6: Invoke { handler: 2 }     -- c

handlers: [a, b, c]
```

### Nested pipe

```
Input:  Pipe([Invoke(a), Pipe([Invoke(b), Invoke(c)]), Invoke(d)])

entries:
  0: Pipe { count: 3 }
  1: ChildRef { action: 4 }       -- a
  2: ChildRef { action: 5 }       -- inner pipe
  3: ChildRef { action: 10 }      -- d
  4: Invoke { handler: 0 }        -- a
  5: Pipe { count: 2 }
  6: ChildRef { action: 8 }       -- b
  7: ChildRef { action: 9 }       -- c
  8: Invoke { handler: 1 }        -- b
  9: Invoke { handler: 2 }        -- c
  10: Invoke { handler: 3 }       -- d

handlers: [a, b, c, d]
```

### Single-child nodes

```
Input:  Loop(Attempt(ForEach(Invoke(process))))

entries:
  0: Loop                      -- body at 1
  1: Attempt                   -- child at 2
  2: ForEach                   -- body at 3
  3: Invoke { handler: 0 }    -- process
```

### Branch

```
Input:  Branch({ "Err": Invoke(handle_err), "Ok": Invoke(handle_ok) })

entries:
  0: Branch { count: 2 }
  1: BranchKey { key: "Err" }
  2: ChildRef { action: 5 }     -- handle_err root
  3: BranchKey { key: "Ok" }
  4: ChildRef { action: 6 }     -- handle_ok root
  5: Invoke { handler: 0 }      -- handle_err
  6: Invoke { handler: 1 }      -- handle_ok

(2 cases × 2 entries each = 4 inline entries after Branch)
```

### Step resolution

```
Input:
  workflow: Pipe([Invoke(setup), Step(Named("Fix"))])
  steps: { Fix: Loop(Invoke(check)) }

Pass 1:
  entries:
    0: Pipe { count: 2 }
    1: ChildRef { action: 3 }
    2: ChildRef { action: 4 }
    3: Invoke { handler: 0 }              -- setup
    4: Step { target: Named("Fix") }
    5: Loop                                -- step "Fix" root
    6: Invoke { handler: 1 }              -- check, body at 5+1

  step_roots: { "Fix" => 5 }

Pass 2:
  entries[4]: Step { target: 5 }
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

/// Pipe: instruction + inline ChildRefs + child entries.
fn flatten_pipe()
// entries: [Pipe{3}, ChildRef(4), ChildRef(5), ChildRef(6), Invoke(0), Invoke(1), Invoke(2)]

/// Parallel: same layout as Pipe but with Parallel variant.
fn flatten_parallel()

/// ForEach: body at self+1.
fn flatten_foreach()
// entries: [ForEach, Invoke(0)]

/// Branch: instruction + 2*count inline entries + case subtrees.
fn flatten_branch()
// entries: [Branch{2}, BranchKey("Err"), ChildRef(5), BranchKey("Ok"), ChildRef(6),
//           Invoke(0), Invoke(1)]

/// Loop: body at self+1.
fn flatten_loop()
// entries: [Loop, Invoke(0)]

/// Attempt: child at self+1.
fn flatten_attempt()
// entries: [Attempt, Invoke(0)]
```

### Nesting

```rust
/// Nested pipe: inner ChildRefs + subtrees interleaved with outer.
fn flatten_nested_pipe()

/// Single-child chain: Loop > Attempt > ForEach.
fn flatten_single_child_chain()
// entries: [Loop, Attempt, ForEach, Invoke(0)]

/// Deep nesting: Attempt > Loop > Pipe.
fn flatten_deep_nesting()

/// Pipe inside Parallel inside Loop.
fn flatten_nested_combinators()

/// Branch cases containing compound subtrees.
fn flatten_branch_with_subtrees()

/// Parallel containing Parallels.
fn flatten_parallel_of_parallels()
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
/// Single-child pipe.
fn flatten_single_child_pipe()

/// Single-case branch.
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
