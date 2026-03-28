# Frame Storage and Advance

Implementation plan for the first engine milestone: store frames and expand an `(ActionId, Value)` cursor into a tree of frames with pending dispatches.

**Depends on:** ENGINE.md (design), flat.rs (FlatConfig, FlatAction, ActionId, etc.)

**Scope:** Frame storage, the `advance` function (including `complete` as a private dependency — needed for empty ForEach/Parallel), `Engine::new`, `Engine::start`, `Engine::take_pending_dispatches`. Task correlation (`TaskId`, `task_to_frame`, `on_task_completed`) and terminal result (`EngineResult`, `result`) are a separate step.

## The advance/complete cycle

The engine operates in a two-phase cycle:

1. **advance(ActionId, Value, parent)** — expand an ActionId into frames. Walks the flat table, creates frames for structural combinators, and bottoms out at Invoke leaves with pending dispatches. This is "given a cursor into the flat table, build the frame tree until everything is waiting on external work."

2. **complete(ParentRef, Value)** — a child finished; advance the parent frame. Reads the frame, decides what to do next (Chain: trampoline to rest, Loop: re-enter or break, Parallel: fill a result slot), and may call `advance()` to expand the next subtree.

The cycle:
```
start(input)
  → creates Root frame
  → advance(workflow_root, input, SingleChild { root_id })
    → creates frames, produces dispatches
      → dispatches go out to runtime

on_task_completed(task_id, result)        ← runtime delivers result
  → finds Invoke frame, calls complete(parent, value)
    → parent frame decides what to do
      → may call advance(next_action_id, value, parent)
        → creates more frames, produces more dispatches
          → dispatches go out to runtime

... repeat until engine.result is Some ...
```

`advance` takes an ActionId (not a frame) because it *creates* frames — it's the "expand this position in the flat table" primitive. `complete` takes a ParentRef (a frame reference) because it *processes* existing frames — it's the "this frame's child delivered a value" primitive. They're dual operations that call each other.

This milestone implements `advance`, `start`, and the frame storage. The next milestone implements `complete`, `error`, and `on_task_completed`.

## Where it lives

New module `engine` in `barnum_event_loop`:

- `crates/barnum_event_loop/src/engine.rs` — pure state machine, no I/O, no async
- The existing `EngineApplier` stub in `lib.rs` will eventually own an `Engine`, but wiring that up is out of scope here

`barnum_event_loop` already depends on `barnum_ast`. No new crate needed.

## Types

All in `engine.rs`. Private to the module except `Engine`, `Dispatch`, and the public API methods.

### FrameId

```rust
/// Key into the engine's frame slab. Wraps the `usize` returned by `Slab::insert`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct FrameId(usize);
```

`FrameId` wraps `usize` (slab keys are `usize`). Not a `u32_newtype` — different underlying type. Slab reuses keys, so FrameIds are not monotonic.

`TaskId` (monotonic `u32` for correlating dispatches to completions) is added in the completion milestone. This milestone doesn't need it — we only produce dispatches, never consume results.

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
    /// Sentinel: the workflow entry point. Only one per engine.
    /// When its child completes or errors, the engine is done.
    Root,
    /// Leaf: handler dispatched, waiting for result.
    Invoke,
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

`ParentRef` is `Copy` — it's two `usize`s (or a `usize` + a `usize`). Stored inline in the child frame. The parent frame is looked up in the slab when the child completes.

Only the Root frame has `parent: None`. Every other frame has `Some(ParentRef)`. `complete` and `error` take `ParentRef` (not `Option`) — the `None` case is handled by Root's own match arm inside `complete_single` / `error`.

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

### Dispatch

```rust
#[derive(Debug)]
pub struct Dispatch {
    pub handler_id: HandlerId,
    pub value: Value,
}
```

`Dispatch` carries `HandlerId` (not the resolved `HandlerKind`). The caller resolves it via `Engine::handler(HandlerId)`.

No `task_id` in this milestone — task correlation (mapping a completed result back to its Invoke frame) is added in the completion milestone along with `TaskId`, `task_to_frame`, and `on_task_completed`.

`EngineResult` is also deferred — in this milestone, we don't expose whether the engine reached a terminal state. Root completion just removes the frame silently.

### Engine

```rust
pub struct Engine {
    flat_config: FlatConfig,
    frames: Slab<Frame>,
    pending_dispatches: Vec<Dispatch>,
}
```

`frames: Slab<Frame>` — the frame store (from the `slab` crate). A `Vec<T>` with a free list: insert returns an opaque key (the index), remove puts the slot on the free list for reuse. O(1) insert/remove/lookup, no hashing, memory reuse. A million-iteration loop reuses the same ~2 slots.

`FrameId` wraps the `usize` key returned by `Slab::insert`.

`pending_dispatches: Vec<Dispatch>` — accumulated during advance. Drained by the caller via `take_pending_dispatches()`.

Fields added in the completion milestone: `task_to_frame: HashMap<TaskId, FrameId>` (maps pending tasks to Invoke frames), `next_task_id: u32` (monotonic counter), `result: Option<EngineResult>` (terminal state).

## Private helpers

```rust
impl Engine {
    fn insert_frame(&mut self, frame: Frame) -> FrameId {
        FrameId(self.frames.insert(frame))
    }
}
```

`Slab::insert` returns a `usize` key and handles free list reuse internally. `FrameId` wraps this key. `Slab::remove(key)` returns the value and frees the slot for reuse.

## advance

The core expansion function. Takes `(ActionId, Value, ParentRef)`, walks the flat table, creates frames for structural combinators, and bottoms out at Invoke leaves with pending dispatches.

Every `advance` call originates from an existing frame. The initial call comes from the Root frame created by `start()`.

Called from:
- `start()` — creates Root frame, then `advance(workflow_root, input, SingleChild { root_id })`
- `complete_single()` — Chain trampoline, Loop re-enter (future milestone)

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

```rust
fn advance(&mut self, action_id: ActionId, value: Value, parent: ParentRef) {
    match self.flat_config.action(action_id) {
        FlatAction::Invoke { handler } => {
            self.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::Invoke,
            });
            self.pending_dispatches.push(Dispatch {
                handler_id: handler,
                value,
            });
        }

        FlatAction::Chain { rest } => {
            let first = self.flat_config.chain_first(action_id);
            let frame_id = self.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::Chain { rest },
            });
            self.advance(first, value, ParentRef::SingleChild { frame_id });
        }

        FlatAction::Parallel { count } => {
            if count.0 == 0 {
                // No children — vacuously complete with empty array.
                self.complete(parent, Value::Array(vec![]));
                return;
            }
            let children: Vec<ActionId> =
                self.flat_config.parallel_children(action_id).collect();
            let frame_id = self.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::Parallel {
                    results: vec![None; count.0 as usize],
                },
            });
            for (i, child) in children.into_iter().enumerate() {
                self.advance(
                    child,
                    value.clone(),
                    ParentRef::IndexedChild { frame_id, child_index: i },
                );
            }
        }

        FlatAction::ForEach { body } => {
            let elements = match value {
                Value::Array(elements) => elements,
                other => panic!("ForEach expected array, got {other}"),
            };
            if elements.is_empty() {
                // No elements — vacuously complete with empty array.
                self.complete(parent, Value::Array(vec![]));
                return;
            }
            let frame_id = self.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::ForEach {
                    results: vec![None; elements.len()],
                },
            });
            for (i, element) in elements.into_iter().enumerate() {
                self.advance(
                    body,
                    element,
                    ParentRef::IndexedChild { frame_id, child_index: i },
                );
            }
        }

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

        FlatAction::Loop { body } => {
            let frame_id = self.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::Loop { body },
            });
            self.advance(body, value, ParentRef::SingleChild { frame_id });
        }

        FlatAction::Attempt { child } => {
            let frame_id = self.insert_frame(Frame {
                parent: Some(parent),
                kind: FrameKind::Attempt,
            });
            self.advance(child, value, ParentRef::SingleChild { frame_id });
        }

        FlatAction::Step { target } => {
            self.advance(target, value, parent);
        }
    }
}
```

**Empty ForEach/Parallel:** When ForEach receives `[]` or Parallel has 0 children, no frame is created. Instead, advance immediately calls `complete(parent, Value::Array(vec![]))` — the empty result propagates upward, potentially triggering further advance calls (e.g., Chain trampoline). No stuck frames.

Branch uses `key.lookup()` (from `intern::Lookup` on `KindDiscriminator`) rather than `key.as_str()`. ENGINE.md uses `key.as_str()` which needs to be updated to match.

`body` is `Copy` (`ActionId` is a `u32` newtype), so it's captured from the pattern match and remains valid after `value` is consumed.

## complete (private, needed by advance)

`advance` calls `complete` for the empty ForEach/Parallel edge cases. The empty result propagates upward through the frame tree — Chain trampolines to rest (calling advance again), Root silently removes itself, etc. This means complete must be fully implemented even though it's not part of the public API in this milestone.

No-op stub. Called by advance for empty ForEach/Parallel — the value is discarded. The full implementation is in COMPLETION.md.

```rust
fn complete(&mut self, _parent_ref: ParentRef, _value: Value) {
    // No-op in the advance milestone. The completion milestone
    // fills this in with the full advance/complete cycle.
}
```

## Public API (this milestone)

```rust
impl Engine {
    /// Create a new engine from a flattened config.
    pub fn new(flat_config: FlatConfig) -> Self {
        Self {
            flat_config,
            frames: Slab::new(),
            pending_dispatches: Vec::new(),
        }
    }

    /// Begin execution. Creates the Root frame and advances from the workflow root.
    pub fn start(&mut self, input: Value) {
        let root_id = self.insert_frame(Frame {
            parent: None,
            kind: FrameKind::Root,
        });
        let workflow_root = self.flat_config.workflow_root();
        self.advance(workflow_root, input, ParentRef::SingleChild { frame_id: root_id });
    }

    /// Drain all pending dispatches accumulated since the last call.
    pub fn take_pending_dispatches(&mut self) -> Vec<Dispatch> {
        std::mem::take(&mut self.pending_dispatches)
    }

    /// Look up a handler by ID. Used by the caller to resolve `Dispatch::handler_id`.
    pub fn handler(&self, id: HandlerId) -> &HandlerKind {
        self.flat_config.handler(id)
    }
}
```

`start` takes a `Value` input rather than hardcoding `Value::Null`. The caller decides the initial value.

Added in the completion milestone: `is_done()`, `result()`, `on_task_completed()`.

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
    /// complete is a no-op, so the empty result is discarded.
    /// The completion milestone will test that empty ForEach propagates correctly.
    #[test]
    fn foreach_empty_array() {
        let mut engine = engine_from(for_each(invoke("./handler.ts", "run")));
        engine.start(json!([]));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 0);
    }

    /// Parallel with empty children: no dispatches.
    /// complete is a no-op, so the empty result is discarded.
    #[test]
    fn parallel_empty() {
        let mut engine = engine_from(parallel(vec![]));
        engine.start(json!(null));

        let dispatches = engine.take_pending_dispatches();
        assert_eq!(dispatches.len(), 0);
    }
}
```

### Not tested here (completion milestone)

- Task correlation (TaskId, on_task_completed)
- Chain trampoline on external completion
- Parallel/ForEach result collection from external completions
- Loop Continue/Break
- Attempt Ok/Err wrapping
- Error propagation
- Terminal result (EngineResult, is_done)
