# Frame Storage and Advance

Implementation plan for the first engine milestone: store frames and expand an `(ActionId, Value)` cursor into a tree of frames with pending dispatches.

**Depends on:** ENGINE.md (design), flat.rs (FlatConfig, FlatAction, ActionId, etc.)

**Scope:** Frame storage, the `advance` function, `Engine::new`, `Engine::start`, `Engine::take_pending_dispatches`. Completion handling (`complete`, `error`, `on_task_completed`) is a separate step.

## Where it lives

New module `engine` in `barnum_event_loop`:

- `crates/barnum_event_loop/src/engine.rs` — pure state machine, no I/O, no async
- The existing `EngineApplier` stub in `lib.rs` will eventually own an `Engine`, but wiring that up is out of scope here

`barnum_event_loop` already depends on `barnum_ast`. No new crate needed.

## Types

All in `engine.rs`. Private to the module except `Engine`, `Dispatch`, `EngineResult`, and the public API methods.

### FrameId, TaskId

```rust
/// Key into the engine's frame slab. Wraps the `usize` returned by `Slab::insert`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FrameId(usize);

u32_newtype!(
    /// Identifies a pending handler invocation. Assigned by the engine,
    /// returned to the engine in `on_task_completed`.
    TaskId
);
```

`FrameId` wraps `usize` (slab keys are `usize`). Not a `u32_newtype` — different underlying type. Slab reuses keys, so FrameIds are not monotonic.

`TaskId` remains a monotonic `u32` counter. Used as a HashMap key for `task_to_frame` and as an external identifier returned in dispatches.

### Frame, ParentRef, FrameKind

```rust
struct Frame {
    parent: Option<ParentRef>,
    kind: FrameKind,
}

#[derive(Debug, Clone, Copy)]
enum ParentRef {
    /// Parent has one active child (Chain, Loop, Attempt).
    SingleChild { frame_id: FrameId },
    /// Parent has N children; this child occupies `child_index` (Parallel, ForEach).
    IndexedChild { frame_id: FrameId, child_index: usize },
}

enum FrameKind {
    /// Leaf: handler dispatched, waiting for result.
    Invoke { task_id: TaskId },
    /// Sequential: first child active, then trampoline to `rest`.
    Chain { rest: ActionId },
    /// Fan-out: collecting results from N parallel branches.
    Parallel { results: Vec<Option<Value>> },
    /// Fan-out: collecting results from N array elements.
    ForEach { results: Vec<Option<Value>> },
    /// Fixed-point: re-enter body on Continue, complete on Break.
    Loop { body: ActionId },
    /// Error boundary: wraps child result in Ok/Err.
    Attempt,
}
```

`ParentRef` is `Copy` — it's two `u32`s (or two `u32`s + a `usize`). Stored inline in the child frame. The parent frame is looked up in the HashMap when the child completes.

`ParentRef` needs a `frame_id()` accessor for `error()`:

```rust
impl ParentRef {
    const fn frame_id(self) -> FrameId {
        match self {
            ParentRef::SingleChild { frame_id } | ParentRef::IndexedChild { frame_id, .. } => {
                frame_id
            }
        }
    }
}
```

### Dispatch, EngineResult

```rust
#[derive(Debug)]
pub struct Dispatch {
    pub task_id: TaskId,
    pub handler_id: HandlerId,
    pub value: Value,
}

#[derive(Debug)]
pub enum EngineResult {
    Success(Value),
    Failure(String),
}
```

`Dispatch` carries `HandlerId` (not the resolved `HandlerKind`). The caller resolves it via `Engine::handler(HandlerId)`.

### Engine

```rust
pub struct Engine {
    flat_config: FlatConfig,
    frames: Slab<Frame>,
    task_to_frame: HashMap<TaskId, FrameId>,
    pending_dispatches: Vec<Dispatch>,
    next_task_id: u32,
    result: Option<EngineResult>,
}
```

`frames: Slab<Frame>` — the frame store (from the `slab` crate). A `Vec<T>` with a free list: insert returns an opaque key (the index), remove puts the slot on the free list for reuse. O(1) insert/remove/lookup, no hashing, memory reuse. A million-iteration loop reuses the same ~2 slots.

`FrameId` wraps the `usize` key returned by `Slab::insert`.

`task_to_frame: HashMap<TaskId, FrameId>` — maps pending task IDs to their Invoke frames. Populated during advance, consumed during `on_task_completed`.

`pending_dispatches: Vec<Dispatch>` — accumulated during advance. Drained by the caller via `take_pending_dispatches()`.

## Private helpers

```rust
impl Engine {
    fn insert_frame(&mut self, frame: Frame) -> FrameId {
        FrameId(self.frames.insert(frame))
    }

    fn next_task_id(&mut self) -> TaskId {
        let task_id = TaskId(self.next_task_id);
        self.next_task_id += 1;
        task_id
    }
}
```

`Slab::insert` returns a `usize` key and handles free list reuse internally. `FrameId` wraps this key. `Slab::remove(key)` returns the value and frees the slot for reuse.

## advance

The core expansion function. Takes `(ActionId, Value, Option<ParentRef>)`, walks the flat table, creates frames for structural combinators, and bottoms out at Invoke leaves with pending dispatches.

Called from:
- `start()` — initial entry: `advance(workflow_root, input, None)`
- `complete_single()` — Chain trampoline, Loop re-enter (future milestone)
- `complete()` with `None` parent — workflow done (future milestone)

### Per-variant behavior

| FlatAction | Creates frame? | Recursive calls |
|---|---|---|
| Invoke | Yes (leaf) | None — bottoms out |
| Chain | Yes | 1 (first child) |
| Parallel | Yes | N (one per child) |
| ForEach | Yes | N (one per array element) |
| Loop | Yes | 1 (body) |
| Attempt | Yes | 1 (child) |
| Branch | No — pass-through | 1 (matching case) |
| Step | No — pass-through | 1 (target) |

Branch and Step are tail calls. They redirect to another ActionId without creating a frame, passing the parent reference through unchanged.

### Value cloning

Parallel clones the input value for each child. ForEach moves each array element. Chain, Loop, Attempt, Branch, Step pass the value through without cloning. Invoke consumes the value into the Dispatch.

### Branch case lookup

`FlatConfig::branch_cases()` returns an iterator of `(KindDiscriminator, ActionId)`. The engine extracts `value["kind"]` as a `&str` and finds the matching case:

```rust
FlatAction::Branch { .. } => {
    let kind_str = value["kind"]
        .as_str()
        .expect("Branch input must have a string 'kind' field");
    let case_action_id = self
        .flat_config
        .branch_cases(action_id)
        .find(|(key, _)| key.lookup() == kind_str)
        .expect("no matching branch case")
        .1;
    self.advance(case_action_id, value, parent);
}
```

Uses `intern::Lookup::lookup()` on `KindDiscriminator` to get `&'static str` for comparison with the runtime `&str`.

### ForEach input validation

ForEach expects `Value::Array`. If the input is not an array, that's a bug in the workflow definition (the type system in TS should prevent this). For now: panic. Later: propagate as an engine error.

### Full implementation

See ENGINE.md `advance` section — the code there is implementation-ready. The only change is the Branch case lookup using `key.lookup()` instead of `key.as_str()`.

## Public API (this milestone)

```rust
impl Engine {
    /// Create a new engine from a flattened config.
    pub fn new(flat_config: FlatConfig) -> Self {
        Self {
            flat_config,
            frames: Slab::new(),
            task_to_frame: HashMap::new(),
            pending_dispatches: Vec::new(),
            next_task_id: 0,
            result: None,
        }
    }

    /// Begin execution. Advances from the workflow root with the given input.
    pub fn start(&mut self, input: Value) {
        let workflow_root = self.flat_config.workflow_root();
        self.advance(workflow_root, input, None);
    }

    /// Drain all pending dispatches accumulated since the last call.
    pub fn take_pending_dispatches(&mut self) -> Vec<Dispatch> {
        std::mem::take(&mut self.pending_dispatches)
    }

    /// Look up a handler by ID. Used by the caller to resolve `Dispatch::handler_id`.
    pub fn handler(&self, id: HandlerId) -> &HandlerKind {
        self.flat_config.handler(id)
    }

    /// Whether the engine has reached a terminal state.
    pub fn is_done(&self) -> bool {
        self.result.is_some()
    }

    /// The terminal result, if the engine is done.
    pub fn result(&self) -> Option<&EngineResult> {
        self.result.as_ref()
    }
}
```

`start` takes a `Value` input rather than hardcoding `Value::Null`. The caller decides the initial value.

## Tests

Tests use the tree AST → `flatten()` → `Engine::new()` → `start()` → inspect dispatches pattern. No completion handling yet — these tests only verify the expansion phase.

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use barnum_ast::flat::flatten;
    use barnum_ast::*;

    // -- Helpers --

    fn ts_handler(module: &str, func: &str) -> HandlerKind {
        HandlerKind::TypeScript(TypeScriptHandler {
            module: ModulePath::from(module.intern()),
            func: FuncName::from(func.intern()),
            step_config_schema: None,
            value_schema: None,
        })
    }

    fn invoke(module: &str, func: &str) -> Action {
        Action::Invoke(InvokeAction {
            handler: ts_handler(module, func),
        })
    }

    fn chain(first: Action, rest: Action) -> Action {
        Action::Chain(ChainAction {
            first: Box::new(first),
            rest: Box::new(rest),
        })
    }

    fn parallel(actions: Vec<Action>) -> Action {
        Action::Parallel(ParallelAction { actions })
    }

    fn for_each(action: Action) -> Action {
        Action::ForEach(ForEachAction {
            action: Box::new(action),
        })
    }

    fn branch(cases: Vec<(&str, Action)>) -> Action {
        Action::Branch(BranchAction {
            cases: cases
                .into_iter()
                .map(|(k, v)| (KindDiscriminator::from(k.intern()), v))
                .collect(),
        })
    }

    fn loop_action(body: Action) -> Action {
        Action::Loop(LoopAction {
            body: Box::new(body),
        })
    }

    fn attempt(action: Action) -> Action {
        Action::Attempt(AttemptAction {
            action: Box::new(action),
        })
    }

    fn step_named(name: &str) -> Action {
        Action::Step(StepAction {
            step: StepRef::Named {
                name: StepName::from(name.intern()),
            },
        })
    }

    fn engine_from(workflow: Action) -> Engine {
        let config = Config {
            workflow,
            steps: HashMap::new(),
        };
        Engine::new(flatten(config).unwrap())
    }

    fn engine_from_config(config: Config) -> Engine {
        Engine::new(flatten(config).unwrap())
    }

    // -- Tests --

    /// Single invoke: start → 1 dispatch.
    #[test]
    fn single_invoke() {
        let mut engine = engine_from(invoke("./handler.ts", "run"));
        engine.start(json!({"x": 1}));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].value, json!({"x": 1}));
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./handler.ts", "run"),
        );
    }

    /// Chain(A, B): only A is dispatched on start.
    #[test]
    fn chain_dispatches_first_only() {
        let mut engine = engine_from(chain(
            invoke("./a.ts", "a"),
            invoke("./b.ts", "b"),
        ));
        engine.start(json!(null));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./a.ts", "a"),
        );
    }

    /// Parallel(A, B, C): all 3 dispatched on start, all receive the same input.
    #[test]
    fn parallel_dispatches_all() {
        let mut engine = engine_from(parallel(vec![
            invoke("./a.ts", "a"),
            invoke("./b.ts", "b"),
            invoke("./c.ts", "c"),
        ]));
        engine.start(json!({"shared": true}));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 3);
        for d in &dispatches {
            assert_eq!(d.value, json!({"shared": true}));
        }
    }

    /// ForEach over 3-element array: 3 dispatches, one per element.
    #[test]
    fn foreach_dispatches_per_element() {
        let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
        engine.start(json!([10, 20, 30]));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 3);
        assert_eq!(dispatches[0].value, json!(10));
        assert_eq!(dispatches[1].value, json!(20));
        assert_eq!(dispatches[2].value, json!(30));
    }

    /// Branch: only the matching case is dispatched.
    #[test]
    fn branch_dispatches_matching_case() {
        let mut engine = engine_from(branch(vec![
            ("Ok", invoke("./ok.ts", "handle")),
            ("Err", invoke("./err.ts", "handle")),
        ]));
        engine.start(json!({"kind": "Ok", "value": 42}));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./ok.ts", "handle"),
        );
    }

    /// Loop: body is dispatched on start.
    #[test]
    fn loop_dispatches_body() {
        let mut engine = engine_from(loop_action(invoke("./handler.ts", "run")));
        engine.start(json!("init"));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].value, json!("init"));
    }

    /// Attempt: child is dispatched on start.
    #[test]
    fn attempt_dispatches_child() {
        let mut engine = engine_from(attempt(invoke("./handler.ts", "run")));
        engine.start(json!("input"));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(dispatches[0].value, json!("input"));
    }

    /// Step(Named): follows the step reference to the target action.
    #[test]
    fn step_follows_named() {
        let config = Config {
            workflow: step_named("setup"),
            steps: HashMap::from([(
                StepName::from("setup".intern()),
                invoke("./setup.ts", "run"),
            )]),
        };
        let mut engine = engine_from_config(config);
        engine.start(json!(null));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./setup.ts", "run"),
        );
    }

    /// Nested: Chain inside Parallel. Parallel(Chain(A, B), C) → dispatches A and C.
    #[test]
    fn nested_chain_in_parallel() {
        let mut engine = engine_from(parallel(vec![
            chain(invoke("./a.ts", "a"), invoke("./b.ts", "b")),
            invoke("./c.ts", "c"),
        ]));
        engine.start(json!(null));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 2);
        // A (first of chain) and C (direct parallel child).
        let handlers: Vec<_> = dispatches
            .iter()
            .map(|d| engine.handler(d.handler_id).clone())
            .collect();
        assert!(handlers.contains(&ts_handler("./a.ts", "a")));
        assert!(handlers.contains(&ts_handler("./c.ts", "c")));
        // B is not dispatched yet (behind Chain).
        assert!(!handlers.contains(&ts_handler("./b.ts", "b")));
    }

    /// Deep chain: Chain(A, Chain(B, C)) → only A dispatched.
    #[test]
    fn deep_chain_dispatches_first_only() {
        let mut engine = engine_from(chain(
            invoke("./a.ts", "a"),
            chain(invoke("./b.ts", "b"), invoke("./c.ts", "c")),
        ));
        engine.start(json!(null));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 1);
        assert_eq!(
            engine.handler(dispatches[0].handler_id),
            &ts_handler("./a.ts", "a"),
        );
    }

    /// ForEach with empty array: no dispatches.
    /// (Engine creates a ForEach frame with 0-length results, which is immediately
    /// "all filled" — but completion handling is a later milestone. For now, just
    /// verify no dispatches are produced.)
    #[test]
    fn foreach_empty_array() {
        let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
        engine.start(json!([]));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 0);
    }
}
```

### Open question: ForEach with empty array

When ForEach receives `[]`, it creates a frame with `results: vec![]` and dispatches nothing. No child will ever complete, so the frame is stuck. The `complete_indexed` function checks `results.iter().all(is_some)` which is vacuously true for an empty vec — so in the completion milestone, we need to handle this edge case in `advance` itself: if elements is empty, skip frame creation and immediately call `complete(parent, Value::Array(vec![]))`.

For this milestone, we just verify no dispatches are produced. The stuck-frame issue will be addressed when completion is implemented.

### Not tested here (completion milestone)

- Chain trampoline on completion
- Parallel/ForEach result collection
- Loop Continue/Break
- Attempt Ok/Err wrapping
- Error propagation
- Task completion flow
