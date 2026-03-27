# Flattening

The nested `Config` (tree of `Action` nodes with step references by name) is flattened into a `FlatConfig`: a bytecode-like linear array of instructions where all cross-references are `ActionId` indices. No heap allocation within action variants.

## Relationship to bytecode

This is a bytecode representation, not SSA. SSA is about variable naming (each variable assigned exactly once for use-def chains). Our flat config has no variables: data flows implicitly through execution. What we have is an **instruction array with index-based operands**: each instruction has an address (ActionId), references other instructions by address, and multi-operand instructions reference contiguous address ranges. Branch discriminator keys live in a side table (analogous to a constant pool).

The BFS layout ensures that children of any node occupy contiguous indices, enabling range references (`first_child + count`) instead of `Vec<ActionId>`.

## Types

**Note:** `ActionId` will be a `u32` newtype via a `u32_newtype!` macro (modeled after isograph's `u64_newtypes` crate). To be created when implementation starts.

```rust
/// A single instruction in the flat action table.
///
/// Generic over `T`: the Step target type. During pass 1, `T = StepTarget`
/// (may contain unresolved step names). After pass 2, `T = ActionId`
/// (fully resolved). The generic applies only to Step -- all other
/// ActionId references are resolved during BFS allocation.
enum FlatAction<T> {
    /// Leaf: invoke a handler.
    Invoke { handler: HandlerKind },

    /// Sequential composition.
    /// Children at actions[first_child .. first_child + count].
    Pipe { first_child: ActionId, count: u32 },

    /// Fan-out: same input to all children, collect results as array.
    /// Children at actions[first_child .. first_child + count].
    Parallel { first_child: ActionId, count: u32 },

    /// Map over array input. Single body action.
    ForEach { body: ActionId },

    /// Case analysis on value["kind"].
    /// Case actions at actions[first_case .. first_case + count].
    /// Discriminator keys at branch_keys[keys_start .. keys_start + count].
    /// branch_keys[keys_start + i] is the discriminator for actions[first_case + i].
    Branch { first_case: ActionId, keys_start: u32, count: u32 },

    /// Fixed-point iteration. Single body action.
    Loop { body: ActionId },

    /// Error materialization. Single child action.
    Attempt { action: ActionId },

    /// Redirect to another action (step reference or self-recursion).
    Step { target: T },
}

/// The fully-resolved flat configuration.
struct FlatConfig {
    /// The instruction array. ActionIds are indices into this vec.
    actions: Vec<FlatAction<ActionId>>,

    /// Branch discriminator keys. Each Branch references a contiguous slice.
    branch_keys: Vec<KindDiscriminator>,

    /// Entry point for execution.
    workflow_root: ActionId,
}
```

Branch key lookup is a linear scan of the relevant `branch_keys` slice. Case counts are small, so this is cache-friendly.

```rust
fn lookup_branch_case(
    flat: &FlatConfig,
    first_case: ActionId,
    keys_start: u32,
    count: u32,
    kind: &KindDiscriminator,
) -> Option<ActionId> {
    let keys = &flat.branch_keys[keys_start as usize..(keys_start + count) as usize];
    keys.iter()
        .position(|k| k == kind)
        .map(|i| ActionId(first_case.0 + i as u32))
}
```

## Algorithm

### Why BFS

Multi-child actions (Pipe, Parallel, Branch) reference their children as a contiguous range `[first_child, first_child + count)`. BFS achieves this naturally: when processing a node, allocate ActionIds for all children as a contiguous block, then enqueue children for later processing. Children are processed in the order they were enqueued (FIFO), so each node's push index equals its pre-allocated ActionId. No indexed insertion needed.

DFS would interleave children's subtrees, breaking contiguity.

### Two-pass resolution

Step(Named(name)) references a named step whose root ActionId may not be allocated yet (it's in a different tree). Resolving step names requires two passes.

**Pass 1**: BFS-flatten each tree, allocating ActionIds. Step(Named(name)) stores the name as `StepTarget::Named(name)`. Step(Root) resolves immediately to `StepTarget::Resolved(workflow_root)` since the workflow root is always the first ActionId allocated.

**Pass 2**: Walk the vec, replacing `StepTarget::Named(name)` with `step_roots[name]`. The result is `Vec<FlatAction<ActionId>>` -- all references resolved. The type system enforces this.

```rust
/// Intermediate state during pass 1. Only Step targets can be unresolved.
enum StepTarget {
    /// Named step reference. Resolved in pass 2.
    Named(StepName),
    /// Already resolved (Root, or after pass 2 completes).
    Resolved(ActionId),
}
```

### Pass 1: BFS flatten

```
flatten_pass1(config) -> (Vec<FlatAction<StepTarget>>, Vec<KindDiscriminator>,
                          HashMap<StepName, ActionId>, ActionId):
    let actions = Vec::new()
    let branch_keys = Vec::new()
    let step_roots = HashMap::new()
    let next_id: u32 = 0
    let queue = VecDeque::new()

    fn alloc() -> ActionId:
        let id = ActionId(next_id)
        next_id += 1
        id

    fn alloc_n(n: usize) -> ActionId:
        let first = ActionId(next_id)
        next_id += n as u32
        first

    // 1. Flatten workflow. Root is always the first ActionId.
    let workflow_root = alloc()
    queue.push_back((workflow_root, config.workflow))
    drain_queue()

    // 2. Flatten each named step.
    for (name, step_action) in config.steps:
        let step_root = alloc()
        step_roots.insert(name, step_root)
        queue.push_back((step_root, step_action))
        drain_queue()

    return (actions, branch_keys, step_roots, workflow_root)


drain_queue():
    while let Some((id, node)) = queue.pop_front():
        match node:
            Invoke { handler } =>
                actions.push(Invoke { handler })

            Pipe { children } =>
                let first = alloc_n(children.len())
                actions.push(Pipe { first_child: first, count: children.len() })
                for (i, child) in children.iter().enumerate():
                    queue.push_back((ActionId(first.0 + i as u32), child))

            Parallel { children } =>
                // identical to Pipe
                let first = alloc_n(children.len())
                actions.push(Parallel { first_child: first, count: children.len() })
                for (i, child) in children.iter().enumerate():
                    queue.push_back((ActionId(first.0 + i as u32), child))

            ForEach { body } =>
                let body_id = alloc()
                actions.push(ForEach { body: body_id })
                queue.push_back((body_id, *body))

            Branch { cases } =>
                let sorted_cases: Vec<_> = cases.into_iter()
                    .sorted_by_key(|(k, _)| k.clone())  // deterministic order
                    .collect()
                let first = alloc_n(sorted_cases.len())
                let keys_start = branch_keys.len() as u32
                for (i, (key, case_action)) in sorted_cases.into_iter().enumerate():
                    branch_keys.push(key)
                    queue.push_back((ActionId(first.0 + i as u32), case_action))
                actions.push(Branch { first_case: first, keys_start, count })

            Loop { body } =>
                let body_id = alloc()
                actions.push(Loop { body: body_id })
                queue.push_back((body_id, *body))

            Attempt { action } =>
                let action_id = alloc()
                actions.push(Attempt { action: action_id })
                queue.push_back((action_id, *action))

            Step(Named(name)) =>
                actions.push(Step { target: StepTarget::Named(name) })

            Step(Root) =>
                actions.push(Step { target: StepTarget::Resolved(workflow_root) })
```

Branch cases are sorted by key for deterministic output. Without sorting, HashMap iteration order is nondeterministic, making tests flaky and the flat config non-reproducible.

### Pass 2: resolve step names

```
resolve(
    actions: Vec<FlatAction<StepTarget>>,
    step_roots: &HashMap<StepName, ActionId>,
) -> Vec<FlatAction<ActionId>>:
    actions.into_iter().map(|action| match action {
        Step { target: StepTarget::Named(name) } =>
            Step { target: *step_roots.get(&name).expect("unknown step: {name}") }
        Step { target: StepTarget::Resolved(id) } =>
            Step { target: id }
        other => other  // map identity over the ActionId fields
    }).collect()
```

## Examples

### Simple pipe

```
Input:
    config(pipe([invoke("a"), invoke("b"), invoke("c")]))

Flat:
    0: Pipe { first_child: 1, count: 3 }
    1: Invoke(a)
    2: Invoke(b)
    3: Invoke(c)
```

### Nested pipe

```
Input:
    config(pipe([invoke("a"), pipe([invoke("b"), invoke("c")]), invoke("d")]))

BFS allocation:
    0: Pipe (root)           alloc children -> 1, 2, 3
    1: Invoke(a)
    2: Pipe (inner)          alloc children -> 4, 5
    3: Invoke(d)
    4: Invoke(b)
    5: Invoke(c)

Flat:
    0: Pipe { first_child: 1, count: 3 }
    1: Invoke(a)
    2: Pipe { first_child: 4, count: 2 }
    3: Invoke(d)
    4: Invoke(b)
    5: Invoke(c)
```

Outer Pipe children: 1, 2, 3 -- contiguous. Inner Pipe children: 4, 5 -- contiguous.

### Step resolution

```
Input:
    workflow: Pipe([Invoke(setup), Step(Named("Fix")), Invoke(cleanup)])
    steps: { Fix: Loop(Invoke(check)) }

Pass 1 (BFS):
    0: Pipe { first_child: 1, count: 3 }
    1: Invoke(setup)
    2: Step { target: Named("Fix") }
    3: Invoke(cleanup)
    4: Loop { body: 5 }           -- step "Fix" root
    5: Invoke(check)

    step_roots: { "Fix" => 4 }

Pass 2:
    2: Step { target: 4 }         -- resolved
```

### Mutual recursion

```
Input:
    workflow: Step(Named("A"))
    steps: {
        A: Pipe([Invoke(do_a), Step(Named("B"))]),
        B: Pipe([Invoke(do_b), Step(Named("A"))]),
    }

Pass 1 flattens each tree independently. Step nodes store names, never follow targets.
Pass 2 resolves names to ActionIds that already exist. No infinite loop.
```

## Unit tests

Tests follow the pipeline: build `Config` (from JSON or programmatically) -> flatten -> assert `FlatConfig`.

### Helpers

```rust
fn ts_handler(name: &str) -> HandlerKind { /* TypeScript handler with module="/m", func=name */ }
fn invoke(name: &str) -> Action { Action::Invoke(InvokeAction { handler: ts_handler(name) }) }
fn pipe(actions: Vec<Action>) -> Action { Action::Pipe(PipeAction { actions }) }
fn parallel(actions: Vec<Action>) -> Action { Action::Parallel(ParallelAction { actions }) }
fn for_each(action: Action) -> Action { Action::ForEach(ForEachAction { action: Box::new(action) }) }
fn branch(cases: Vec<(&str, Action)>) -> Action { /* sorted by key */ }
fn loop_(body: Action) -> Action { Action::Loop(LoopAction { body: Box::new(body) }) }
fn attempt(action: Action) -> Action { Action::Attempt(AttemptAction { action: Box::new(action) }) }
fn step_named(name: &str) -> Action { Action::Step(StepAction { step: StepRef::Named { name: name.into() } }) }
fn step_root() -> Action { Action::Step(StepAction { step: StepRef::Root }) }
fn config(workflow: Action) -> Config { Config { workflow, steps: HashMap::new() } }
fn config_with_steps(workflow: Action, steps: Vec<(&str, Action)>) -> Config { /* ... */ }
fn flatten(config: Config) -> FlatConfig { /* pass 1 + pass 2 */ }
```

### Basic structure tests

```rust
/// Single invoke: one instruction, root = 0.
fn flatten_single_invoke()
// [0: Invoke(handler)]

/// Pipe: children occupy contiguous range.
fn flatten_pipe_contiguous()
// [0: Pipe{1,3}, 1: Invoke(a), 2: Invoke(b), 3: Invoke(c)]

/// Parallel: children occupy contiguous range.
fn flatten_parallel_contiguous()
// [0: Parallel{1,2}, 1: Invoke(a), 2: Invoke(b)]

/// ForEach: single body reference.
fn flatten_foreach()
// [0: ForEach{body:1}, 1: Invoke(process)]

/// Branch: cases contiguous, keys in side table, sorted by key.
fn flatten_branch()
// [0: Branch{first:1,keys:0,count:2}, 1: Invoke(err), 2: Invoke(ok)]
// branch_keys: ["Error", "Ok"]

/// Loop: single body reference.
fn flatten_loop()
// [0: Loop{body:1}, 1: Invoke(check)]

/// Attempt: single child reference.
fn flatten_attempt()
// [0: Attempt{action:1}, 1: Invoke(risky)]
```

### Nesting tests

```rust
/// BFS contiguity: nested pipe children don't break parent contiguity.
fn flatten_nested_pipe()
// [0: Pipe{1,3}, 1: Invoke(a), 2: Pipe{4,2}, 3: Invoke(d), 4: Invoke(b), 5: Invoke(c)]

/// Deep nesting: attempt > loop > pipe.
fn flatten_deep_nesting()
// [0: Attempt{1}, 1: Loop{2}, 2: Pipe{3,2}, 3: Invoke(a), 4: Invoke(b)]

/// Pipe inside parallel inside loop.
fn flatten_nested_combinators()
// [0: Loop{1}, 1: Parallel{2,2}, 2: Pipe{4,2}, 3: Invoke(c), 4: Invoke(a), 5: Invoke(b)]

/// Branch containing pipes.
fn flatten_branch_with_subtrees()
// Branch cases are subtrees, not just invokes. Children contiguous.

/// Parallel of parallels.
fn flatten_parallel_of_parallels()
// Inner parallel children contiguous, outer parallel children contiguous.
```

### Step resolution tests

```rust
/// Step(Root) resolved immediately to ActionId(0).
fn flatten_step_root()
// [0: Pipe{1,2}, 1: Invoke(a), 2: Step{target:0}]

/// Named step resolved in pass 2.
fn flatten_step_named()
// Pipe([Invoke(a), Step("Fix")]) + steps { Fix: Invoke(fix) }
// [0: Pipe{1,2}, 1: Invoke(a), 2: Step{target:3}, 3: Invoke(fix)]

/// Mutual recursion: A -> B -> A, no infinite loop.
fn flatten_mutual_recursion()
// Both steps flattened independently, targets resolved correctly.

/// Self-recursion: step body contains Step(Root).
fn flatten_self_recursion()
// Step(Root) inside a step body resolves to workflow root, not step root.

/// Chain of steps: A -> B -> C -> Invoke.
fn flatten_chain_of_steps()
// Each Step resolved to its target's root.
```

### JSON round-trip tests

```rust
/// Parse JSON -> deserialize Config -> flatten -> assert.
fn flatten_from_json_simple_pipe()
// JSON: { "workflow": { "kind": "Pipe", "actions": [...] } }

/// JSON workflow with named steps.
fn flatten_from_json_with_steps()

/// JSON with Branch.
fn flatten_from_json_branch()

/// Complex workflow exercising all action types.
fn flatten_from_json_kitchen_sink()
```

### Edge cases

```rust
/// Single-child pipe (degenerate but valid).
fn flatten_single_child_pipe()
// [0: Pipe{1,1}, 1: Invoke(a)]

/// Branch with single case.
fn flatten_single_case_branch()

/// Unknown step name panics.
#[should_panic(expected = "unknown step")]
fn flatten_unknown_step_panics()

/// Deterministic output: flattening the same config twice yields identical results.
fn flatten_deterministic()
// Run flatten twice, assert byte-for-byte equality.
```
