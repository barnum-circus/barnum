# Flattening

The nested `Config` (tree of `Action` nodes with step references by name) is flattened into a `FlatConfig`: a linear array of entries where all cross-references are `ActionId` indices. No heap allocation within entry variants.

## Types

**Note:** `ActionId` and `HandlerId` will be `u32` newtypes via a `u32_newtype!` macro (modeled after isograph's `u64_newtypes` crate). `DiscriminatorId` will be a `u16` newtype. To be created when implementation starts.

```rust
/// An entry in the flat action table.
///
/// Multi-child instructions (Pipe, Parallel, Branch) are followed by
/// `count` inline operand entries at self+1.
///   - Pipe, Parallel: `count` Item entries.
///   - Branch: `count` BranchCase entries.
///
/// Single-child instructions (ForEach, Loop, Attempt) have their child
/// at self+1 unconditionally. No ActionId field needed.
///
/// Generic over `T`: the Step target type. During pass 1, `T = StepTarget`
/// (may contain unresolved step names). After pass 2, `T = ActionId`
/// (fully resolved). The generic applies only to Step.
enum FlatEntry<T> {
    // -- Instructions --

    /// Leaf: invoke a handler. `handler` indexes into `FlatConfig::handlers`.
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
    /// Followed by `count` BranchCase entries. Each has a discriminator key + case root.
    Branch { count: u16 },

    /// Fixed-point iteration. Body at self+1.
    Loop,

    /// Error materialization. Child at self+1.
    Attempt,

    /// Redirect to another action (step reference or self-recursion).
    Step { target: T },

    // -- Inline operands (always read in context of a parent instruction) --

    /// Child reference for Pipe or Parallel. Points to a child's root entry.
    Item { action: ActionId },

    /// Branch case: discriminator key + case root action.
    /// `key` is a DiscriminatorId (u16) indexing into `FlatConfig::discriminators`.
    /// `action` is the ActionId of the case subtree's root.
    /// Payload: u16 + u32 = 6 bytes. Fits in 8 bytes with discriminant.
    BranchCase { key: DiscriminatorId, action: ActionId },
}
```

### Entry size

Largest variant payload: `BranchCase` at u16 + u32 = 6 bytes. With a 1-byte discriminant, every entry fits in 8 bytes. Enforce with `static_assert!(size_of::<FlatEntry<ActionId>>() <= 8)`.

Side tables for heap types:

1. **Handler pool.** `Invoke` stores a `HandlerId` (u32 index) into `FlatConfig::handlers: Vec<HandlerKind>`. Handlers are interned: identical `HandlerKind` values share the same `HandlerId`.
2. **Discriminator interning.** `BranchCase` stores a `DiscriminatorId` (u16 index) into `FlatConfig::discriminators: Vec<KindDiscriminator>`. Discriminators are interned: identical values share the same `DiscriminatorId`. u16 supports 65,536 unique discriminators.

```rust
/// The fully-resolved flat configuration.
struct FlatConfig {
    /// The entry array. ActionIds are indices into this vec.
    entries: Vec<FlatEntry<ActionId>>,

    /// Entry point for execution.
    workflow_root: ActionId,

    /// Handler pool. HandlerIds are indices into this vec.
    handlers: Vec<HandlerKind>,

    /// Discriminator pool. DiscriminatorIds are indices into this vec.
    discriminators: Vec<KindDiscriminator>,
}
```

Multi-child instructions store only `count`. Pipe and Parallel operand entries are at `self+1` through `self+count`. Branch operand entries are BranchCase entries at `self+1` through `self+count`. Single-child instructions have their child unconditionally at `self+1`. No offset fields.

Branch key lookup: linear scan over the `count` BranchCase entries following the Branch instruction, comparing each `key` against the interned discriminator of the input value's `kind` field.

## Algorithm

### DFS with pre-allocated operand slots

DFS flattening. Multi-child nodes: pre-allocate the instruction slot and `count` operand slots, then DFS into each child and backpatch the operand entries with the child's root ActionId.

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

### Interning

Pass 1 maintains two intern tables (HashMap from value to id). When a handler or discriminator is encountered, look it up; if absent, push to the pool and assign the next id.

```
intern_handler(handler: &HandlerKind) -> HandlerId:
    if let Some(id) = handler_map.get(handler):
        return *id
    let id = HandlerId(handlers.len() as u32)
    handlers.push(handler.clone())
    handler_map.insert(handler.clone(), id)
    id

intern_discriminator(key: &KindDiscriminator) -> DiscriminatorId:
    if let Some(id) = discriminator_map.get(key):
        return *id
    let id = DiscriminatorId(discriminators.len() as u16)
    discriminators.push(key.clone())
    discriminator_map.insert(key.clone(), id)
    id
```

### Pass 1: DFS flatten

```
flatten_pass1(config) -> (Vec<FlatEntry<StepTarget>>, HashMap<StepName, ActionId>, ActionId,
                          Vec<HandlerKind>, Vec<KindDiscriminator>):
    let entries: Vec<Option<FlatEntry<StepTarget>>> = Vec::new()
    let step_roots = HashMap::new()
    let next_id: u32 = 0
    let handlers: Vec<HandlerKind> = Vec::new()
    let handler_map: HashMap<HandlerKind, HandlerId> = HashMap::new()
    let discriminators: Vec<KindDiscriminator> = Vec::new()
    let discriminator_map: HashMap<KindDiscriminator, DiscriminatorId> = HashMap::new()

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
                let handler_id = intern_handler(handler)
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
                let cases_start = alloc_n(sorted.len())
                entries[id] = Some(Branch { count: sorted.len() as u16 })
                for (i, (key, case_action)) in sorted.iter().enumerate():
                    let disc_id = intern_discriminator(key)
                    let case_root = flatten_node(case_action)
                    entries[cases_start + i] = Some(BranchCase { key: disc_id, action: case_root })
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
    (entries, step_roots, workflow_root, handlers, discriminators)
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
Input:  Pipe([Invoke(a), Invoke(b), Invoke(c)])

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
Input:  Pipe([Invoke(a), Pipe([Invoke(b), Invoke(c)]), Invoke(d)])

  0: Pipe { count: 3 }
  1: Item { action: 4 }       -- child 0: Invoke(a)
  2: Item { action: 5 }       -- child 1: inner Pipe
  3: Item { action: 10 }      -- child 2: Invoke(d)
  4: Invoke(a)
  5: Pipe { count: 2 }
  6: Item { action: 8 }
  7: Item { action: 9 }
  8: Invoke(b)
  9: Invoke(c)
  10: Invoke(d)
```

### Single-child nodes

```
Input:  Loop(Attempt(ForEach(Invoke(process))))

  0: Loop                      -- body at 1
  1: Attempt                   -- child at 2
  2: ForEach                   -- body at 3
  3: Invoke(process)
```

### Branch

```
Input:  Branch({ "Err": Invoke(handle_err), "Ok": Invoke(handle_ok) })

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
  5: Loop                          -- step "Fix" root
  6: Invoke(check)                 -- Loop body at 5+1

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

/// Parallel: same layout as Pipe but with Parallel instruction.
fn flatten_parallel()

/// ForEach: body at self+1.
fn flatten_foreach()
// [0: ForEach, 1: Invoke(process)]

/// Branch: instruction + count BranchCases + case subtrees.
fn flatten_branch()
// [0: Branch{2}, 1: BranchCase{"Err",3}, 2: BranchCase{"Ok",4}, 3: Invoke(err), 4: Invoke(ok)]

/// Branch cases sorted by key for determinism.
fn flatten_branch_deterministic()

/// Loop: body at self+1.
fn flatten_loop()
// [0: Loop, 1: Invoke(check)]

/// Attempt: child at self+1.
fn flatten_attempt()
// [0: Attempt, 1: Invoke(risky)]
```

### Nesting

```rust
/// Nested pipe: inner pipe Items point deeper into the array.
fn flatten_nested_pipe()

/// Single-child chain: Loop > Attempt > ForEach.
fn flatten_single_child_chain()
// [0: Loop, 1: Attempt, 2: ForEach, 3: Invoke(x)]

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

/// Discriminator interning: identical keys share the same DiscriminatorId.
fn flatten_discriminator_interning()

/// Static assert: FlatEntry<ActionId> fits in 8 bytes.
fn flat_entry_size()
```
