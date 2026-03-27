# Engine Design

The engine interprets the Action AST. Two responsibilities:

1. **Advance** — Given a cursor (location + value from a completed handler), walk the flat action table to produce the next set of handler dispatches. Pure state manipulation.
2. **Schedule** — Dispatch pending handler invocations to an executor with concurrency control. I/O.

## Step 1: Flatten the AST

The nested `Config` (tree of `Action` nodes) is flattened into a `FlatConfig`: a `Vec<FlatAction>` where every node has an `ActionId` (index). All references between actions are `ActionId`s, not nested pointers.

```rust
type ActionId = u32;

struct FlatConfig {
    actions: Vec<FlatAction>,
    /// The workflow entry point.
    workflow: ActionId,
}

enum FlatAction {
    Invoke { handler: HandlerKind },
    Pipe { actions: Vec<ActionId> },
    Parallel { actions: Vec<ActionId> },
    ForEach { body: ActionId },
    Branch { cases: HashMap<KindDiscriminator, ActionId> },
    Loop { body: ActionId },
    Attempt { action: ActionId },
    /// Resolved step reference. Points to the step's root action.
    Step { target: ActionId },
}
```

Flattening walks the nested `Config` depth-first, assigns each node an `ActionId`, and resolves Step references:

- `Step(Named("FixCycle"))` → look up `config.steps["FixCycle"]`, flatten that subtree, store its root `ActionId` as the target.
- `Step(Root)` → target is `FlatConfig::workflow`.
- Mutual recursion works because both steps are in the same flat vec and reference each other by `ActionId`.

Validation happens during flattening. If a step name doesn't exist, flattening fails — not runtime. The engine never panics on bad step references.

### Example

Nested:
```
workflow: Pipe([
    Invoke(setup),
    Step(Named("FixCycle")),
])
steps: {
    FixCycle: Loop(Invoke(healthCheck))
}
```

Flattened:
```
0: Pipe { actions: [1, 2] }         ← workflow (ActionId 0)
1: Invoke(setup)
2: Step { target: 3 }               ← resolved FixCycle
3: Loop { body: 4 }                 ← FixCycle root
4: Invoke(healthCheck)
```

## Step 2: Engine and cursors

A **cursor** is `(ActionId, Value)` — a position in the flat action table plus the value produced by the preceding handler. The engine processes one cursor at a time: each handler completion produces one cursor, which the engine advances until it hits the next set of handler invocations.

The engine's core method:

```rust
fn advance_from_cursor(&mut self, location: ActionId, value: Value)
```

This walks the action table from `location`, advancing through structural combinators until it reaches Invoke nodes (which queue dispatches) or must wait for child results.

### Hidden root handler

Execution starts with a synthetic completion: the "root handler" completes with `Value::Null`. This feeds the first cursor into `advance_from_cursor(workflow, null)`. After that, everything is driven by real handler completions. The engine's event handling is uniform — startup is just another completion.

### Frames

While cursors are external inputs (one per handler completion), the engine needs internal state to track partial progress through structural combinators. A **frame** tracks this per-node state:

```rust
struct Frame {
    id: FrameId,
    parent: Option<ParentRef>,
    kind: FrameKind,
}

struct ParentRef {
    frame_id: FrameId,
    /// Child's position within the parent (e.g., index in Parallel results).
    index: usize,
}

enum FrameKind {
    /// Leaf: handler dispatched, waiting for result.
    Invoke { task_id: TaskId },

    /// Sequential: at action `index` within the Pipe's action list.
    Pipe { action_id: ActionId, index: usize },

    /// Collecting results from N parallel branches.
    Parallel { action_id: ActionId, results: Vec<Option<Value>>, remaining: usize },

    /// Collecting results from N array elements.
    ForEach { action_id: ActionId, results: Vec<Option<Value>>, remaining: usize },

    /// Fixed-point iteration.
    Loop { action_id: ActionId },

    /// Error materialization.
    Attempt { action_id: ActionId },

    /// Step or Branch delegation. Forwards child result to parent.
    Passthrough,
}
```

Frames reference actions by `ActionId`. No cloned AST nodes, no tree navigation.

### Engine state

```rust
struct Engine {
    config: FlatConfig,
    frames: HashMap<FrameId, Frame>,
    task_to_frame: HashMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    next_frame_id: u64,
    next_task_id: u64,
    result: Option<EngineResult>,
}

struct Dispatch {
    task_id: TaskId,
    handler: HandlerKind,
    value: Value,
}
```

### advance_from_cursor(location, value)

Walk the action table from `location` with `value`:

| FlatAction | Behavior |
|------------|----------|
| **Invoke** | Create Invoke frame. Queue dispatch(task_id, handler, value). |
| **Pipe** | Create Pipe frame (index=0). Recurse into actions[0]. |
| **Parallel** | Create Parallel frame (remaining=N). Recurse into each action. |
| **ForEach** | Value must be array. Create ForEach frame (remaining=len). Recurse into body for each element. |
| **Branch** | Read value["kind"]. Look up case. Create Passthrough frame. Recurse into case's ActionId. |
| **Loop** | Create Loop frame. Recurse into body. |
| **Attempt** | Create Attempt frame. Recurse into inner action. |
| **Step** | Create Passthrough frame. Recurse into target ActionId. |

### complete(frame_id, value)

A child resolved. Advance the parent:

| Parent kind | Behavior |
|-------------|----------|
| **Pipe** | Increment index. If more actions, recurse into next. Otherwise complete parent. |
| **Parallel** | Store result at index. Decrement remaining. If 0, collect array, complete parent. |
| **ForEach** | Same as Parallel. |
| **Loop** | If value.kind == "Continue": recurse into body again. If "Break": complete parent. |
| **Attempt** | Wrap as {kind: "Ok", value}. Complete parent. |
| **Passthrough** | Forward to parent. |

### on_task_completed(task_id, result)

Look up the Invoke frame. Remove it. Call complete or error on the parent.

## Worked example

FlatConfig for `pipe(constant({project: "test"}), setup(), build())`:
```
0: Pipe { actions: [1, 2, 3] }
1: Invoke(constant)
2: Invoke(setup)
3: Invoke(build)
```

**Synthetic root completion → advance_from_cursor(0, null):**
```
action[0] = Pipe → create F1 (Pipe, action_id=0, index=0)
action[1] = Invoke(constant) → create F2 (Invoke, t1)
queue dispatch(t1, constant, null)

pending: [t1]
```

**on_task_completed(t1, {project: "test"}):**
```
remove F2. complete(F1, {project: "test"})
F1 Pipe: index 0→1. action[0].actions[1] = ActionId 2.
action[2] = Invoke(setup) → create F3 (Invoke, t2)
queue dispatch(t2, setup, {project: "test"})

pending: [t2]
```

**on_task_completed(t2, {initialized: true, project: "test"}):**
```
remove F3. complete(F1, ...)
F1 Pipe: index 1→2. action[0].actions[2] = ActionId 3.
action[3] = Invoke(build) → create F4 (Invoke, t3)
queue dispatch(t3, build, {initialized: true, project: "test"})
```

**on_task_completed(t3, {artifact: "test.build"}):**
```
remove F4. complete(F1, ...)
F1 Pipe: index 2→3. 3 == len. Pipe complete.
F1 is root → result = Success({artifact: "test.build"})
```

## Testing strategy

### Priority 1: Flatten

Test that `FlatConfig::from(config)` produces the correct flat structure.

```rust
#[test]
fn flatten_pipe() {
    let config = Config {
        workflow: Action::Pipe(PipeAction {
            actions: vec![invoke_action("a"), invoke_action("b")],
        }),
        steps: HashMap::new(),
    };
    let flat = FlatConfig::from(config);
    assert!(matches!(flat.actions[flat.workflow], FlatAction::Pipe { .. }));
    // Pipe's children are Invoke nodes
    let FlatAction::Pipe { actions } = &flat.actions[flat.workflow] else { panic!() };
    assert_eq!(actions.len(), 2);
    assert!(matches!(flat.actions[actions[0]], FlatAction::Invoke { .. }));
    assert!(matches!(flat.actions[actions[1]], FlatAction::Invoke { .. }));
}

#[test]
fn flatten_resolves_step_references() {
    let config = Config {
        workflow: Action::Step(StepAction {
            step: StepRef::Named { name: "MyStep".into() },
        }),
        steps: HashMap::from([
            ("MyStep".into(), invoke_action("handler")),
        ]),
    };
    let flat = FlatConfig::from(config);
    let FlatAction::Step { target } = flat.actions[flat.workflow] else { panic!() };
    assert!(matches!(flat.actions[target], FlatAction::Invoke { .. }));
}

#[test]
fn flatten_mutual_recursion() {
    // Steps A and B reference each other
    let config = Config {
        workflow: Action::Step(StepAction {
            step: StepRef::Named { name: "A".into() },
        }),
        steps: HashMap::from([
            ("A".into(), Action::Pipe(PipeAction {
                actions: vec![
                    invoke_action("doA"),
                    Action::Step(StepAction { step: StepRef::Named { name: "B".into() } }),
                ],
            })),
            ("B".into(), Action::Pipe(PipeAction {
                actions: vec![
                    invoke_action("doB"),
                    Action::Step(StepAction { step: StepRef::Named { name: "A".into() } }),
                ],
            })),
        ]),
    };
    let flat = FlatConfig::from(config);
    // Both steps exist and reference each other
    // The Step nodes' targets should point to each other's roots
    assert!(flat.actions.len() >= 6); // workflow Step, A's Pipe+Invoke+Step, B's Pipe+Invoke+Step
}
```

### Priority 2: Advance (dispatch generation)

Test that `advance_from_cursor` produces the correct dispatches.

```rust
#[test]
fn advance_single_invoke() {
    let mut engine = engine_from(Config {
        workflow: invoke_action("setup"),
        steps: HashMap::new(),
    });
    // Synthetic root completion
    engine.advance_from_cursor(engine.config.workflow, Value::Null);

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].handler_func(), "setup");
    assert_eq!(d[0].value, Value::Null);
}

#[test]
fn advance_pipe_dispatches_first_action() {
    let mut engine = engine_from(Config {
        workflow: Action::Pipe(PipeAction {
            actions: vec![invoke_action("a"), invoke_action("b"), invoke_action("c")],
        }),
        steps: HashMap::new(),
    });
    engine.advance_from_cursor(engine.config.workflow, Value::Null);

    // Only the first action is dispatched
    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].handler_func(), "a");
}

#[test]
fn advance_parallel_dispatches_all() {
    let mut engine = engine_from(Config {
        workflow: Action::Parallel(ParallelAction {
            actions: vec![invoke_action("x"), invoke_action("y"), invoke_action("z")],
        }),
        steps: HashMap::new(),
    });
    engine.advance_from_cursor(engine.config.workflow, Value::Null);

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 3);
}

#[test]
fn advance_foreach_dispatches_per_element() {
    let mut engine = engine_from(Config {
        workflow: Action::ForEach(ForEachAction {
            action: Box::new(invoke_action("process")),
        }),
        steps: HashMap::new(),
    });
    engine.advance_from_cursor(
        engine.config.workflow,
        json!([{"a": 1}, {"a": 2}]),
    );

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 2);
    assert_eq!(d[0].value, json!({"a": 1}));
    assert_eq!(d[1].value, json!({"a": 2}));
}

#[test]
fn advance_branch_dispatches_matching_case() {
    let mut engine = engine_from(Config {
        workflow: Action::Branch(BranchAction {
            cases: HashMap::from([
                ("Yes".into(), invoke_action("accept")),
                ("No".into(), invoke_action("reject")),
            ]),
        }),
        steps: HashMap::new(),
    });
    engine.advance_from_cursor(
        engine.config.workflow,
        json!({"kind": "Yes", "data": 42}),
    );

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].handler_func(), "accept");
}

#[test]
fn pipe_then_complete_advances_to_next() {
    let mut engine = engine_from(Config {
        workflow: Action::Pipe(PipeAction {
            actions: vec![invoke_action("first"), invoke_action("second")],
        }),
        steps: HashMap::new(),
    });
    engine.advance_from_cursor(engine.config.workflow, Value::Null);

    let d = engine.take_pending_dispatches();
    engine.on_task_completed(&d[0].task_id, &success(json!({"x": 1})));

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].handler_func(), "second");
    assert_eq!(d[0].value, json!({"x": 1}));
}

#[test]
fn loop_continue_re_dispatches() {
    let mut engine = engine_from(Config {
        workflow: Action::Loop(LoopAction {
            body: Box::new(invoke_action("check")),
        }),
        steps: HashMap::new(),
    });
    engine.advance_from_cursor(engine.config.workflow, json!({"n": 0}));

    let d = engine.take_pending_dispatches();
    assert_eq!(d[0].value, json!({"n": 0}));

    engine.on_task_completed(&d[0].task_id, &success(json!({
        "kind": "Continue", "value": {"n": 1}
    })));

    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].value, json!({"n": 1}));
}

#[test]
fn loop_break_completes() {
    let mut engine = engine_from(Config {
        workflow: Action::Loop(LoopAction {
            body: Box::new(invoke_action("check")),
        }),
        steps: HashMap::new(),
    });
    engine.advance_from_cursor(engine.config.workflow, json!({"n": 0}));

    let d = engine.take_pending_dispatches();
    engine.on_task_completed(&d[0].task_id, &success(json!({
        "kind": "Break", "value": {"done": true}
    })));

    assert!(engine.is_done());
    assert_eq!(engine.result_value(), Some(&json!({"done": true})));
}
```

Test helpers: `invoke_action(name)` builds an `Action::Invoke`. `engine_from(config)` flattens and constructs. `success(v)` builds `TaskResult::Success`. Keep tests readable.

## Integration with scheduling

Deferred. The engine is a pure state machine — it has no knowledge of concurrency limits, thread pools, or I/O. `take_pending_dispatches()` always returns everything. The scheduler is a separate component that wraps the engine, manages concurrency limits, and decides when to actually invoke handlers.

## Open questions

1. **Should Step be a FlatAction variant, or should it be inlined during flattening?** Keeping it as a variant preserves observability (logs show "entering step FixCycle"). Inlining eliminates a layer of indirection. Leaning toward keeping it for now.

2. **Task ID generation.** Monotonic counter for now. UUID if we need cross-run uniqueness later.
