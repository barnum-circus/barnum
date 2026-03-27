# Flattening

The nested `Config` (tree of `Action` nodes with step references by name) is flattened into a `FlatConfig`: a linear array of entries where all cross-references are `ActionId` indices. No heap allocation within entry variants.

## Types

**Note:** `ActionId`, `HandlerId`, and `BranchTableId` will be `u32` newtypes via a `u32_newtype!` macro (modeled after isograph's `u64_newtypes` crate). To be created when implementation starts.

```rust
/// An entry in the flat action table.
///
/// Pipe and Parallel reference children via a side-table pool
/// (`first_child` + `count` index into `FlatConfig::children`).
///
/// Branch references a dispatch table via a side-table pool
/// (`table` indexes `FlatConfig::branch_tables`).
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
    /// `first_child` + `count` index into `FlatConfig::children`.
    Pipe { first_child: u16, count: u16 },

    /// Fan-out: same input to all children, collect results as array.
    /// `first_child` + `count` index into `FlatConfig::children`.
    Parallel { first_child: u16, count: u16 },

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

Every variant payload is at most 4 bytes (`u32` or `u16 + u16`). Discriminant + payload fits in 8 bytes. Enforce with `static_assert!(size_of::<FlatEntry<ActionId>>() <= 8)`.

```rust
/// The fully-resolved flat configuration.
struct FlatConfig {
    /// The entry array. ActionIds are indices into this vec.
    entries: Vec<FlatEntry<ActionId>>,

    /// Handler pool. HandlerIds are indices into this vec.
    /// Identical handlers are interned to share a HandlerId.
    handlers: Vec<HandlerKind>,

    /// Children pool. Pipe and Parallel reference contiguous slices:
    /// `children[first_child..first_child + count]`.
    /// Each element is an ActionId pointing to a child's root entry.
    children: Vec<ActionId>,

    /// Branch dispatch tables. BranchTableIds are indices into this vec.
    /// Each table maps a KindDiscriminator to the ActionId of the matching case.
    branch_tables: Vec<HashMap<KindDiscriminator, ActionId>>,

    /// Entry point for execution.
    workflow_root: ActionId,
}
```

Single-child instructions (ForEach, Loop, Attempt) have their child unconditionally at `self+1` — guaranteed by DFS ordering.

## Algorithm

### DFS with side-table pools

DFS flattening. Pipe and Parallel push their children's root ActionIds into the children pool and record the slice start and length. Branch builds a HashMap and pushes it to the branch_tables pool.

Single-child nodes (ForEach, Loop, Attempt): allocate the instruction slot, then immediately DFS into the child. Since no other allocation happens between the instruction and the child, the child's root is guaranteed to be at self+1.

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

### Handler interning

Pass 1 maintains an intern table (HashMap from HandlerKind to HandlerId). When a handler is encountered, look it up; if absent, push to the pool and assign the next id.

```
intern_handler(handler: &HandlerKind) -> HandlerId:
    if let Some(id) = handler_map.get(handler):
        return *id
    let id = HandlerId(handlers.len() as u32)
    handlers.push(handler.clone())
    handler_map.insert(handler.clone(), id)
    id
```

KindDiscriminator is already an interned StringKey (Copy, u32-sized). No second interning needed — branch_tables use it directly as HashMap keys.

### Pass 1: DFS flatten

```
flatten_pass1(config) -> (Vec<FlatEntry<StepTarget>>, HashMap<StepName, ActionId>,
                          Vec<HandlerKind>, Vec<ActionId>,
                          Vec<HashMap<KindDiscriminator, ActionId>>, ActionId):
    let entries: Vec<Option<FlatEntry<StepTarget>>> = Vec::new()
    let step_roots = HashMap::new()
    let handlers: Vec<HandlerKind> = Vec::new()
    let handler_map: HashMap<HandlerKind, HandlerId> = HashMap::new()
    let children: Vec<ActionId> = Vec::new()
    let branch_tables: Vec<HashMap<KindDiscriminator, ActionId>> = Vec::new()
    let next_id: u32 = 0

    fn alloc() -> ActionId:
        let id = ActionId(next_id)
        entries.push(None)
        next_id += 1
        id

    fn flatten_node(node: &Action) -> ActionId:
        match node:
            Invoke { handler } =>
                let id = alloc()
                let handler_id = intern_handler(handler)
                entries[id] = Some(Invoke { handler: handler_id })
                id

            Pipe { actions } =>
                let id = alloc()
                let first_child = children.len() as u16
                let count = actions.len() as u16
                // Reserve slots in children pool
                for _ in 0..count:
                    children.push(ActionId(0))
                entries[id] = Some(Pipe { first_child, count })
                for (i, child) in actions.iter().enumerate():
                    let child_root = flatten_node(child)
                    children[first_child as usize + i] = child_root
                id

            Parallel { actions } =>
                let id = alloc()
                let first_child = children.len() as u16
                let count = actions.len() as u16
                for _ in 0..count:
                    children.push(ActionId(0))
                entries[id] = Some(Parallel { first_child, count })
                for (i, child) in actions.iter().enumerate():
                    let child_root = flatten_node(child)
                    children[first_child as usize + i] = child_root
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
    (entries, step_roots, handlers, children, branch_tables, workflow_root)
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

entries:
  0: Pipe { first_child: 0, count: 3 }
  1: Invoke(handler: 0)        -- a
  2: Invoke(handler: 1)        -- b
  3: Invoke(handler: 2)        -- c

children: [1, 2, 3]
handlers: [a, b, c]
```

### Nested pipe

```
Input:  Pipe([Invoke(a), Pipe([Invoke(b), Invoke(c)]), Invoke(d)])

entries:
  0: Pipe { first_child: 0, count: 3 }
  1: Invoke(handler: 0)       -- a
  2: Pipe { first_child: 3, count: 2 }
  3: Invoke(handler: 1)       -- b
  4: Invoke(handler: 2)       -- c
  5: Invoke(handler: 3)       -- d

children: [1, 2, 5, 3, 4]
           ^------^  outer pipe children[0..3] → entries 1, 2, 5
                   ^--^  inner pipe children[3..5] → entries 3, 4
handlers: [a, b, c, d]
```

### Single-child nodes

```
Input:  Loop(Attempt(ForEach(Invoke(process))))

entries:
  0: Loop                      -- body at 1
  1: Attempt                   -- child at 2
  2: ForEach                   -- body at 3
  3: Invoke(handler: 0)        -- process
```

### Branch

```
Input:  Branch({ "Err": Invoke(handle_err), "Ok": Invoke(handle_ok) })

entries:
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
  entries:
    0: Pipe { first_child: 0, count: 2 }
    1: Invoke(handler: 0)              -- setup
    2: Step { target: Named("Fix") }
    3: Loop                            -- step "Fix" root
    4: Invoke(handler: 1)              -- check, body at 3+1

  children: [1, 2]
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
// entries: [Invoke(0)]

/// Pipe: entries + children pool.
fn flatten_pipe()
// entries: [Pipe{0,3}, Invoke(0), Invoke(1), Invoke(2)]
// children: [1, 2, 3]

/// Parallel: same layout as Pipe but with Parallel instruction.
fn flatten_parallel()

/// ForEach: body at self+1.
fn flatten_foreach()
// entries: [ForEach, Invoke(0)]

/// Branch: single entry + branch_table. Case subtrees follow in entries.
fn flatten_branch()
// entries: [Branch{table:0}, Invoke(0), Invoke(1)]
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
/// Nested pipe: children pool interleaves outer and inner slices.
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
