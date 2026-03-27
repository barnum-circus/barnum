# Flattening

The nested `Config` (tree of `Action` nodes with step references by name) is flattened into a `FlatConfig`: a linear array of entries where all cross-references are `ActionId` indices. No heap allocation within entry variants.

## Types

**Note:** `ActionId` will be a `u32` newtype via a `u32_newtype!` macro (modeled after isograph's `u64_newtypes` crate). To be created when implementation starts.

```rust
/// An entry in the flat action table. Instructions and inline operands.
///
/// Multi-child instructions (Pipe, Parallel, Branch) are followed by
/// `count` inline operand entries (Item or BranchCase). The operands
/// are always at positions self_id+1 through self_id+count.
///
/// Generic over `T`: the Step target type. During pass 1, `T = StepTarget`
/// (may contain unresolved step names). After pass 2, `T = ActionId`
/// (fully resolved). The generic applies only to Step.
enum FlatEntry<T> {
    // -- Instructions --

    /// Leaf: invoke a handler.
    Invoke { handler: HandlerKind },

    /// Sequential composition.
    /// Followed by `count` Item entries at self+1..self+1+count.
    Pipe { count: u32 },

    /// Fan-out: same input to all children, collect results as array.
    /// Followed by `count` Item entries at self+1..self+1+count.
    Parallel { count: u32 },

    /// Map over array input. Single body action.
    ForEach { body: ActionId },

    /// Case analysis on value["kind"].
    /// Followed by `count` BranchCase entries at self+1..self+1+count.
    Branch { count: u32 },

    /// Fixed-point iteration. Single body action.
    Loop { body: ActionId },

    /// Error materialization. Single child action.
    Attempt { action: ActionId },

    /// Redirect to another action (step reference or self-recursion).
    Step { target: T },

    // -- Inline operands (always read in context of a parent instruction) --

    /// Child reference for Pipe or Parallel. Points to the child's root entry.
    Item { action: ActionId },

    /// Case reference for Branch. Discriminator key + pointer to case root.
    BranchCase { key: KindDiscriminator, action: ActionId },
}

/// The fully-resolved flat configuration.
struct FlatConfig {
    /// The entry array. ActionIds are indices into this vec.
    entries: Vec<FlatEntry<ActionId>>,

    /// Entry point for execution.
    workflow_root: ActionId,
}
```

Multi-child instructions store only `count`. Their operand entries are always at `self+1` through `self+count`. No `first_child`, no `first_case`, no side tables.

Branch key lookup: read the `count` BranchCase entries following the Branch instruction, linear scan for the matching key.

## Algorithm

### DFS with pre-allocated operand slots

DFS flattening. When encountering a multi-child node, pre-allocate the instruction slot and operand slots, then DFS into each child. Backpatch the operand entries with each child's root ActionId.

Single-child nodes (ForEach, Loop, Attempt) DFS into their child and store the returned root ActionId directly in the instruction.

Step(Named(name)) stores the name, resolved in pass 2. Step(Root) resolves immediately since the workflow root is always the first ActionId allocated.

### Two-pass resolution

**Pass 1**: DFS-flatten each tree. Step(Named(name)) stores `StepTarget::Named(name)`. Step(Root) resolves immediately to `StepTarget::Resolved(workflow_root)`.

**Pass 2**: Walk the vec, replacing `StepTarget::Named(name)` with `step_roots[name]`.

```rust
/// Intermediate state during pass 1.
enum StepTarget {
    Named(StepName),
    Resolved(ActionId),
}
```

### Pass 1: DFS flatten

```
flatten_pass1(config) -> (Vec<FlatEntry<StepTarget>>, HashMap<StepName, ActionId>, ActionId):
    let entries: Vec<Option<FlatEntry<StepTarget>>> = Vec::new()
    let step_roots = HashMap::new()
    let next_id: u32 = 0

    fn alloc() -> ActionId:
        let id = ActionId(next_id)
        entries.push(None)       // reserve slot
        next_id += 1
        id

    fn alloc_n(n: u32) -> ActionId:
        let first = ActionId(next_id)
        for _ in 0..n:
            entries.push(None)   // reserve slots
        next_id += n
        first

    // Returns the root ActionId of the flattened subtree.
    fn flatten_node(node: &Action) -> ActionId:
        match node:
            Invoke { handler } =>
                let id = alloc()
                entries[id] = Some(Invoke { handler })
                id

            Pipe { children } =>
                let id = alloc()
                let items_start = alloc_n(children.len())
                entries[id] = Some(Pipe { count: children.len() })
                for (i, child) in children.iter().enumerate():
                    let child_root = flatten_node(child)
                    entries[items_start + i] = Some(Item { action: child_root })
                id

            Parallel { children } =>
                let id = alloc()
                let items_start = alloc_n(children.len())
                entries[id] = Some(Parallel { count: children.len() })
                for (i, child) in children.iter().enumerate():
                    let child_root = flatten_node(child)
                    entries[items_start + i] = Some(Item { action: child_root })
                id

            ForEach { body } =>
                let id = alloc()
                let body_root = flatten_node(body)
                entries[id] = Some(ForEach { body: body_root })
                id

            Branch { cases } =>
                let id = alloc()
                let sorted: Vec<_> = cases.iter()
                    .sorted_by_key(|(k, _)| k)
                    .collect()
                let cases_start = alloc_n(sorted.len())
                entries[id] = Some(Branch { count: sorted.len() })
                for (i, (key, case_action)) in sorted.iter().enumerate():
                    let case_root = flatten_node(case_action)
                    entries[cases_start + i] = Some(BranchCase { key, action: case_root })
                id

            Loop { body } =>
                let id = alloc()
                let body_root = flatten_node(body)
                entries[id] = Some(Loop { body: body_root })
                id

            Attempt { action } =>
                let id = alloc()
                let action_root = flatten_node(action)
                entries[id] = Some(Attempt { action: action_root })
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

    // All slots filled.
    let entries = entries.into_iter().map(Option::unwrap).collect()
    (entries, step_roots, workflow_root)
```

Branch cases are sorted by key for deterministic output.

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
Input:
    Pipe([Invoke(a), Invoke(b), Invoke(c)])

Flat:
    0: Pipe { count: 3 }
    1: Item { action: 4 }
    2: Item { action: 5 }
    3: Item { action: 6 }
    4: Invoke(a)
    5: Invoke(b)
    6: Invoke(c)
```

### Nested pipe

```
Input:
    Pipe([Invoke(a), Pipe([Invoke(b), Invoke(c)]), Invoke(d)])

Flat:
    0: Pipe { count: 3 }
    1: Item { action: 4 }
    2: Item { action: 5 }
    3: Item { action: 10 }
    4: Invoke(a)
    5: Pipe { count: 2 }
    6: Item { action: 8 }
    7: Item { action: 9 }
    8: Invoke(b)
    9: Invoke(c)
    10: Invoke(d)
```

### Branch

```
Input:
    Branch({ "Err": Invoke(handle_err), "Ok": Invoke(handle_ok) })

Flat:
    0: Branch { count: 2 }
    1: BranchCase { key: "Err", action: 3 }
    2: BranchCase { key: "Ok", action: 4 }
    3: Invoke(handle_err)
    4: Invoke(handle_ok)
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
    3: Invoke(setup)
    4: Step { target: Named("Fix") }
    5: Loop { body: 6 }
    6: Invoke(check)

    step_roots: { "Fix" => 5 }

Pass 2:
    4: Step { target: 5 }
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
fn branch(cases: Vec<(&str, Action)>) -> Action { /* sorted by key */ }
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
// [0: Invoke(handler)]

/// Pipe: instruction + count Items + count children.
fn flatten_pipe()
// [0: Pipe{3}, 1: Item{4}, 2: Item{5}, 3: Item{6}, 4: Invoke(a), 5: Invoke(b), 6: Invoke(c)]

/// Parallel: same layout as Pipe.
fn flatten_parallel()

/// ForEach: instruction with inline body ActionId.
fn flatten_foreach()
// [0: ForEach{body:1}, 1: Invoke(process)]

/// Branch: instruction + count BranchCases + case subtrees.
fn flatten_branch()
// [0: Branch{2}, 1: BranchCase{"Err",3}, 2: BranchCase{"Ok",4}, 3: Invoke(err), 4: Invoke(ok)]

/// Branch cases sorted by key for determinism.
fn flatten_branch_deterministic()

/// Loop: instruction with inline body ActionId.
fn flatten_loop()
// [0: Loop{body:1}, 1: Invoke(check)]

/// Attempt: instruction with inline child ActionId.
fn flatten_attempt()
// [0: Attempt{action:1}, 1: Invoke(risky)]
```

### Nesting

```rust
/// Nested pipe: inner pipe Items point deeper into the array.
fn flatten_nested_pipe()
// See nested pipe example above.

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
