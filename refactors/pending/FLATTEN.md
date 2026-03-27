# Flattening

The nested `Config` (tree of `Action` nodes with step references by name) is flattened into a `FlatConfig`: a linear array of entries where all cross-references are `ActionId` indices. No heap allocation within entry variants.

## Types

**Note:** `ActionId`, `HandlerId`, and `BranchTableId` will be `u32` newtypes via a `u32_newtype!` macro (modeled after isograph's `u64_newtypes` crate). To be created when implementation starts.

```rust
/// An entry in the flat action table.
///
/// Multi-child instructions (Pipe, Parallel) are followed by
/// `count` inline Item entries at self+1..self+1+count.
///
/// Single-child instructions (ForEach, Loop, Attempt) have their child
/// at self+1 unconditionally. No ActionId field needed.
///
/// Branch uses a side-table lookup (no inline entries).
///
/// Generic over `T`: the Step target type. During pass 1, `T = StepTarget`
/// (may contain unresolved step names). After pass 2, `T = ActionId`
/// (fully resolved). The generic applies only to Step.
enum FlatEntry<T> {
    // -- Instructions --

    /// Leaf: invoke a handler. HandlerId indexes FlatConfig::handlers.
    Invoke { handler: HandlerId },

    /// Sequential composition.
    /// Followed by `count` Item entries. Each Item points to a child's root.
    Pipe { count: u16 },

    /// Fan-out: same input to all children, collect results as array.
    /// Followed by `count` Item entries. Each Item points to a child's root.
    Parallel { count: u16 },

    /// Map over array input. Body at self+1.
    ForEach,

    /// Case analysis on value["kind"].
    /// BranchTableId indexes FlatConfig::branch_tables for O(1) lookup.
    Branch { table: BranchTableId },

    /// Fixed-point iteration. Body at self+1.
    Loop,

    /// Error materialization. Child at self+1.
    Attempt,

    /// Redirect to another action (step reference or self-recursion).
    Step { target: T },

    // -- Inline operands (always read in context of a parent instruction) --

    /// Child reference for Pipe or Parallel. Points to a child's root entry.
    Item { action: ActionId },
}
```

Every variant payload is at most `u32`. Discriminant + payload fits in 8 bytes. Add `static_assert!(size_of::<FlatEntry<ActionId>>() <= 8)`.

```rust
/// The fully-resolved flat configuration.
struct FlatConfig {
    /// The entry array. ActionIds are indices into this vec.
    entries: Vec<FlatEntry<ActionId>>,

    /// Handler pool. HandlerIds are indices into this vec.
    handlers: Vec<HandlerKind>,

    /// Branch dispatch tables. BranchTableIds are indices into this vec.
    /// Each table maps a discriminator to the ActionId of the matching case.
    branch_tables: Vec<HashMap<KindDiscriminator, ActionId>>,

    /// Entry point for execution.
    workflow_root: ActionId,
}
```

Pipe and Parallel store `count`. Their Item entries are always at `self+1` through `self+count`. Single-child instructions have their child unconditionally at `self+1`. Branch uses a side-table for O(1) dispatch.

## Algorithm

### DFS with pre-allocated operand slots

DFS flattening. Multi-child nodes (Pipe, Parallel): pre-allocate the instruction slot and `count` operand slots, then DFS into each child and backpatch the Item entries with the child's root ActionId.

Single-child nodes (ForEach, Loop, Attempt): allocate the instruction slot, then immediately DFS into the child. Since no other allocation happens between the instruction and the child, the child's root is guaranteed to be at self+1.

Branch: allocate the instruction slot, DFS into each case subtree, build a HashMap mapping discriminator keys to case root ActionIds, push it to the branch_tables pool.

Step(Named(name)) stores the name, resolved in pass 2. Step(Root) resolves immediately since the workflow root is always the first ActionId allocated.

### Two-pass resolution

**Pass 1**: DFS-flatten each tree. Step(Named(name)) stores `StepTarget::Named(name)`. Step(Root) resolves immediately to `StepTarget::Resolved(workflow_root)`.

**Pass 2**: Walk the vec, replacing `StepTarget::Named(name)` with `step_roots[name]`.

```rust
enum StepTarget {
    Named(StepName),
    Resolved(ActionId),
}
```

### Pass 1: DFS flatten

```
flatten_pass1(config) -> (Vec<FlatEntry<StepTarget>>, HashMap<StepName, ActionId>,
                          Vec<HandlerKind>, Vec<HashMap<KindDiscriminator, ActionId>>,
                          ActionId):
    let entries: Vec<Option<FlatEntry<StepTarget>>> = Vec::new()
    let step_roots = HashMap::new()
    let handlers: Vec<HandlerKind> = Vec::new()
    let branch_tables: Vec<HashMap<KindDiscriminator, ActionId>> = Vec::new()
    let next_id: u32 = 0

    fn alloc() -> ActionId:
        let id = ActionId(next_id)
        entries.push(None)
        next_id += 1
        id

    fn alloc_n(n: usize) -> ActionId:
        let first = ActionId(next_id)
        for _ in 0..n:
            entries.push(None)
        next_id += n as u32
        first

    fn flatten_node(node: &Action) -> ActionId:
        match node:
            Invoke { handler } =>
                let id = alloc()
                let handler_id = HandlerId(handlers.len() as u32)
                handlers.push(handler.clone())
                entries[id] = Some(Invoke { handler: handler_id })
                id

            Pipe { children } =>
                let id = alloc()
                let items_start = alloc_n(children.len())
                entries[id] = Some(Pipe { count: children.len() as u16 })
                for (i, child) in children.iter().enumerate():
                    let child_root = flatten_node(child)
                    entries[items_start + i] = Some(Item { action: child_root })
                id

            Parallel { children } =>
                let id = alloc()
                let items_start = alloc_n(children.len())
                entries[id] = Some(Parallel { count: children.len() as u16 })
                for (i, child) in children.iter().enumerate():
                    let child_root = flatten_node(child)
                    entries[items_start + i] = Some(Item { action: child_root })
                id

            ForEach { body } =>
                let id = alloc()
                entries[id] = Some(ForEach)
                flatten_node(body)    // root guaranteed at id+1
                id

            Branch { cases } =>
                let id = alloc()
                let sorted: Vec<_> = cases.iter()
                    .sorted_by_key(|(k, _)| k)
                    .collect()
                let mut table = HashMap::new()
                for (key, case_action) in sorted:
                    let case_root = flatten_node(case_action)
                    table.insert(key.clone(), case_root)
                let table_id = BranchTableId(branch_tables.len() as u32)
                branch_tables.push(table)
                entries[id] = Some(Branch { table: table_id })
                id

            Loop { body } =>
                let id = alloc()
                entries[id] = Some(Loop)
                flatten_node(body)    // root guaranteed at id+1
                id

            Attempt { action } =>
                let id = alloc()
                entries[id] = Some(Attempt)
                flatten_node(action)  // root guaranteed at id+1
                id

            Step(Named(name)) =>
                let id = alloc()
                entries[id] = Some(Step { target: StepTarget::Named(name) })
                id

            Step(Root) =>
                let id = alloc()
                entries[id] = Some(Step { target: StepTarget::Resolved(workflow_root) })
                id

    // 1. Flatten workflow. Root is always the first ActionId.
    let workflow_root = flatten_node(&config.workflow)

    // 2. Flatten each named step.
    for (name, step_action) in &config.steps:
        let step_root = flatten_node(step_action)
        step_roots.insert(name, step_root)

    let entries = entries.into_iter().map(Option::unwrap).collect()
    (entries, step_roots, handlers, branch_tables, workflow_root)
```

Branch cases are sorted by key for deterministic ActionId assignment.

### Pass 2: resolve step names

```
resolve(
    entries: Vec<FlatEntry<StepTarget>>,
    step_roots: &HashMap<StepName, ActionId>,
) -> Vec<FlatEntry<ActionId>>:
    entries.into_iter().map(|entry| match entry {
        Step { target: StepTarget::Named(name) } =>
            Step { target: *step_roots.get(&name).expect("unknown step: {name}") }
        Step { target: StepTarget::Resolved(id) } =>
            Step { target: id }
        other => other
    }).collect()
```

## Examples

### Simple pipe

```
Input:  Pipe([Invoke(a), Invoke(b), Invoke(c)])

  0: Pipe { count: 3 }
  1: Item { action: 4 }
  2: Item { action: 5 }
  3: Item { action: 6 }
  4: Invoke(handler: 0)        -- handlers[0] = a
  5: Invoke(handler: 1)        -- handlers[1] = b
  6: Invoke(handler: 2)        -- handlers[2] = c
```

### Nested pipe

```
Input:  Pipe([Invoke(a), Pipe([Invoke(b), Invoke(c)]), Invoke(d)])

  0: Pipe { count: 3 }
  1: Item { action: 4 }       -- child 0: Invoke(a)
  2: Item { action: 5 }       -- child 1: inner Pipe
  3: Item { action: 10 }      -- child 2: Invoke(d)
  4: Invoke(handler: 0)       -- a
  5: Pipe { count: 2 }
  6: Item { action: 8 }
  7: Item { action: 9 }
  8: Invoke(handler: 1)       -- b
  9: Invoke(handler: 2)       -- c
  10: Invoke(handler: 3)      -- d
```

### Single-child nodes

```
Input:  Loop(Attempt(ForEach(Invoke(process))))

  0: Loop                      -- body at 1
  1: Attempt                   -- child at 2
  2: ForEach                   -- body at 3
  3: Invoke(handler: 0)        -- process
```

### Branch

```
Input:  Branch({ "Err": Invoke(handle_err), "Ok": Invoke(handle_ok) })

  0: Branch { table: 0 }
  1: Invoke(handler: 0)        -- handle_err
  2: Invoke(handler: 1)        -- handle_ok

  branch_tables[0]: { "Err" => 1, "Ok" => 2 }
```

### Step resolution

```
Input:
  workflow: Pipe([Invoke(setup), Step(Named("Fix"))])
  steps: { Fix: Loop(Invoke(check)) }

Pass 1:
  0: Pipe { count: 2 }
  1: Item { action: 3 }
  2: Item { action: 4 }
  3: Invoke(handler: 0)              -- setup
  4: Step { target: Named("Fix") }
  5: Loop                            -- step "Fix" root
  6: Invoke(handler: 1)              -- check, Loop body at 5+1

  step_roots: { "Fix" => 5 }

Pass 2:
  4: Step { target: 5 }
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
// [0: Invoke(0)]

/// Pipe: instruction + count Items + count children.
fn flatten_pipe()
// [0: Pipe{3}, 1: Item{4}, 2: Item{5}, 3: Item{6}, 4: Invoke(0), 5: Invoke(1), 6: Invoke(2)]

/// Parallel: same layout as Pipe but with Parallel instruction.
fn flatten_parallel()

/// ForEach: body at self+1.
fn flatten_foreach()
// [0: ForEach, 1: Invoke(0)]

/// Branch: single entry + side-table. Case subtrees follow.
fn flatten_branch()
// [0: Branch{table:0}, 1: Invoke(0), 2: Invoke(1)]
// branch_tables[0]: { "Err" => 1, "Ok" => 2 }

/// Loop: body at self+1.
fn flatten_loop()
// [0: Loop, 1: Invoke(0)]

/// Attempt: child at self+1.
fn flatten_attempt()
// [0: Attempt, 1: Invoke(0)]
```

### Nesting

```rust
/// Nested pipe: inner pipe Items point deeper into the array.
fn flatten_nested_pipe()

/// Single-child chain: Loop > Attempt > ForEach.
fn flatten_single_child_chain()
// [0: Loop, 1: Attempt, 2: ForEach, 3: Invoke(0)]

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
```
