# Flattening

The nested `Config` (tree of `Action` nodes with step references by name) is flattened into a `FlatConfig`: a linear array of entries where all cross-references are `ActionId` indices. No heap allocation within entry variants.

## Types

**Note:** `ActionId` is a `u32` newtype (indexes the entries array, which can be large). `HandlerId`, `BranchTableId`, and `ChildrenTableId` are `u16` newtypes (side-table indices — 65k unique handlers/tables is more than sufficient). All via a newtype macro (modeled after isograph's `u64_newtypes` crate). To be created when implementation starts.

```rust
/// An entry in the flat action table.
///
/// Multi-child instructions (Pipe, Parallel) reference their children
/// via `FlatConfig::children_tables`, indexed by `ChildrenTableId`.
///
/// Branch references a dispatch table via `FlatConfig::branch_tables`,
/// indexed by `BranchTableId`.
///
/// Single-child instructions (ForEach, Loop, Attempt) have their child
/// at self+1 unconditionally. No field needed.
///
/// Generic over `T`: the Step target type. During pass 1, `T = StepTarget`
/// (may contain unresolved step names). After pass 2, `T = ActionId`
/// (fully resolved). The generic applies only to Step.
enum FlatEntry<T> {
    /// Leaf: invoke a handler. `handler` indexes `FlatConfig::handlers`.
    Invoke { handler: HandlerId },

    /// Sequential composition.
    /// `children` indexes `FlatConfig::children_tables`.
    Pipe { children: ChildrenTableId },

    /// Fan-out: same input to all children, collect results as array.
    /// `children` indexes `FlatConfig::children_tables`.
    Parallel { children: ChildrenTableId },

    /// Map over array input. Body at self+1.
    ForEach,

    /// Case analysis on value["kind"].
    /// `table` indexes `FlatConfig::branch_tables`.
    Branch { table: BranchTableId },

    /// Fixed-point iteration. Body at self+1.
    Loop,

    /// Error materialization. Child at self+1.
    Attempt,

    /// Redirect to another action (step reference or self-recursion).
    Step { target: T },
}
```

Every variant payload is at most `u32`. Step uses `ActionId` (u32); all others use a `u16` newtype. Discriminant + payload fits in 8 bytes. Enforce with `static_assert!(size_of::<FlatEntry<ActionId>>() <= 8)`.

```rust
impl<T> FlatEntry<T> {
    fn map_target<U>(self, f: impl FnOnce(T) -> U) -> FlatEntry<U> {
        match self {
            FlatEntry::Step { target } => FlatEntry::Step { target: f(target) },
            FlatEntry::Invoke { handler } => FlatEntry::Invoke { handler },
            FlatEntry::Pipe { children } => FlatEntry::Pipe { children },
            FlatEntry::Parallel { children } => FlatEntry::Parallel { children },
            FlatEntry::ForEach => FlatEntry::ForEach,
            FlatEntry::Branch { table } => FlatEntry::Branch { table },
            FlatEntry::Loop => FlatEntry::Loop,
            FlatEntry::Attempt => FlatEntry::Attempt,
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
struct FlatConfig {
    /// The entry array. ActionIds are indices into this vec.
    entries: Vec<FlatEntry<ActionId>>,

    /// Handler pool. HandlerIds are indices into this vec.
    /// Handlers are interned: identical handlers share a HandlerId.
    handlers: Vec<HandlerKind>,

    /// Children tables. ChildrenTableIds are indices into this vec.
    /// Each element is a list of ActionIds pointing to children's root entries.
    /// Used by Pipe and Parallel.
    children_tables: Vec<Vec<ActionId>>,

    /// Branch dispatch tables. BranchTableIds are indices into this vec.
    /// Each table maps a KindDiscriminator to the ActionId of the matching case.
    branch_tables: Vec<HashMap<KindDiscriminator, ActionId>>,

    /// Entry point for execution.
    workflow_root: ActionId,
}
```

Single-child instructions (ForEach, Loop, Attempt) have their child unconditionally at `self+1` — guaranteed by DFS ordering.

## Algorithm

### Overview

DFS flattening with side-table pools. Pipe and Parallel collect their children's root ActionIds into a `Vec`, push it to `children_tables`, and store the resulting `ChildrenTableId`. Branch builds a HashMap and pushes it to `branch_tables`.

Single-child nodes (ForEach, Loop, Attempt): allocate the instruction slot, then immediately DFS into the child. Since no other allocation happens between the instruction and the child, the child's root is guaranteed to be at self+1.

Step(Named(name)) stores the name, resolved in pass 2. Step(Root) resolves immediately since the workflow root is always ActionId(0).

Handlers are interned via `IndexSet<HandlerKind>`: identical handlers share a `HandlerId`. `IndexSet::insert_full` returns the index (existing or new), giving O(1) dedup with stable insertion-order indices.

KindDiscriminator is already an interned StringKey (Copy, u32-sized). No second interning needed — branch_tables use it directly as HashMap keys.

### Two-pass resolution

**Pass 1**: DFS-flatten each tree. Step(Named(name)) stores `StepTarget::Named(name)`. Step(Root) resolves immediately to `StepTarget::Resolved(ActionId(0))`.

**Pass 2**: Walk the vec, replacing `StepTarget::Named(name)` with `step_roots[name]` via `map_target`.

### Pass 1: Flattener

```rust
struct Flattener {
    entries: Vec<Option<FlatEntry<StepTarget>>>,
    handlers: IndexSet<HandlerKind>,
    children_tables: Vec<Vec<ActionId>>,
    branch_tables: Vec<HashMap<KindDiscriminator, ActionId>>,
}

impl Flattener {
    fn alloc(&mut self) -> ActionId {
        let id = ActionId(self.entries.len() as u32);
        self.entries.push(None);
        id
    }

    fn intern_handler(&mut self, handler: HandlerKind) -> HandlerId {
        let (index, _) = self.handlers.insert_full(handler);
        HandlerId(index as u16)
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
                let mut child_roots = Vec::with_capacity(actions.len());
                for child in actions {
                    child_roots.push(self.flatten_node(child));
                }
                let table_id =
                    ChildrenTableId(self.children_tables.len() as u16);
                self.children_tables.push(child_roots);
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Pipe { children: table_id });
                id
            }

            Action::Parallel { actions } => {
                let id = self.alloc();
                let mut child_roots = Vec::with_capacity(actions.len());
                for child in actions {
                    child_roots.push(self.flatten_node(child));
                }
                let table_id =
                    ChildrenTableId(self.children_tables.len() as u16);
                self.children_tables.push(child_roots);
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Parallel { children: table_id });
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
                let mut sorted_keys: Vec<_> = cases.keys().collect();
                sorted_keys.sort();
                let mut table = HashMap::new();
                for key in sorted_keys {
                    let case_root = self.flatten_node(&cases[key]);
                    table.insert(key.clone(), case_root);
                }
                let table_id =
                    BranchTableId(self.branch_tables.len() as u16);
                self.branch_tables.push(table);
                self.entries[id.0 as usize] =
                    Some(FlatEntry::Branch { table: table_id });
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
        children_tables: f.children_tables,
        branch_tables: f.branch_tables,
        workflow_root,
    }
}
```

## Examples

### Simple pipe

```
Input:  Pipe([Invoke(a), Invoke(b), Invoke(c)])

entries:
  0: Pipe { children: 0 }
  1: Invoke { handler: 0 }    -- a
  2: Invoke { handler: 1 }    -- b
  3: Invoke { handler: 2 }    -- c

children_tables[0]: [1, 2, 3]
handlers: [a, b, c]
```

### Nested pipe

```
Input:  Pipe([Invoke(a), Pipe([Invoke(b), Invoke(c)]), Invoke(d)])

entries:
  0: Pipe { children: 1 }
  1: Invoke { handler: 0 }       -- a
  2: Pipe { children: 0 }
  3: Invoke { handler: 1 }       -- b
  4: Invoke { handler: 2 }       -- c
  5: Invoke { handler: 3 }       -- d

children_tables[0]: [3, 4]       -- inner pipe (pushed first by DFS)
children_tables[1]: [1, 2, 5]    -- outer pipe
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
  0: Branch { table: 0 }
  1: Invoke { handler: 0 }    -- handle_err
  2: Invoke { handler: 1 }    -- handle_ok

branch_tables[0]: { "Err" => 1, "Ok" => 2 }
```

### Step resolution

```
Input:
  workflow: Pipe([Invoke(setup), Step(Named("Fix"))])
  steps: { Fix: Loop(Invoke(check)) }

Pass 1:
  entries:
    0: Pipe { children: 0 }
    1: Invoke { handler: 0 }              -- setup
    2: Step { target: Named("Fix") }
    3: Loop                                -- step "Fix" root
    4: Invoke { handler: 1 }              -- check, body at 3+1

  children_tables[0]: [1, 2]
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

/// Pipe: entries + children table.
fn flatten_pipe()
// entries: [Pipe { children: 0 }, Invoke(0), Invoke(1), Invoke(2)]
// children_tables[0]: [1, 2, 3]

/// Parallel: same layout as Pipe but with Parallel variant.
fn flatten_parallel()

/// ForEach: body at self+1.
fn flatten_foreach()
// entries: [ForEach, Invoke(0)]

/// Branch: single entry + branch_table. Case subtrees follow in entries.
fn flatten_branch()
// entries: [Branch { table: 0 }, Invoke(0), Invoke(1)]
// branch_tables[0]: { "Err" => 1, "Ok" => 2 }

/// Loop: body at self+1.
fn flatten_loop()
// entries: [Loop, Invoke(0)]

/// Attempt: child at self+1.
fn flatten_attempt()
// entries: [Attempt, Invoke(0)]
```

### Nesting

```rust
/// Nested pipe: inner tables pushed before outer by DFS.
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
