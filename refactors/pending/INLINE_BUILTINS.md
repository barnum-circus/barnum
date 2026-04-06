# Inline Builtins in Advance

**Blocks:** none
**Blocked by:** EVENT_LOOP_RESTRUCTURE (assumed complete)

## Motivation

Builtins are pure synchronous data transformations (`Constant`, `Identity`, `ExtractField`, `Tag`, etc.). They have no I/O, no async, no side effects. Despite this, the engine models them identically to TypeScript subprocess handlers: `advance` creates an Invoke frame, assigns a TaskId, pushes a DispatchEvent onto the pending effects queue, and waits for an external caller to execute the builtin and deliver a CompletionEvent back through `complete()`.

This forces two execution models:

**Production** (`run_workflow` in `barnum_event_loop`): The event loop dispatches builtins to the Scheduler, which spawns a tokio task that calls `execute_builtin`, sends the result through a channel, and the event loop receives it as a CompletionEvent. Pure synchronous functions take an async round-trip through spawn + channel send + channel recv.

**Tests** (`barnum_engine` tests): The `drive_builtins` helper (test_helpers.rs:195) pops pending effects, executes builtins inline via `execute_builtin`, and calls `complete()` with the result. TypeScript dispatches are collected into a Vec for the test to complete manually. Every test that uses builtins calls `drive_builtins` after advance and `complete_and_drive` after completing TypeScript handlers.

The `drive_builtins` helper does the right thing operationally, but it exists because the engine treats builtins as external work that needs to round-trip through the dispatch/complete cycle. Builtins are structurally equivalent to the combinators that `advance` already resolves inline (Chain creates a frame and recurses, Branch picks a case and recurses, ForEach iterates and recurses). A `Constant(42)` is a direct value transformation, not external work.

## Design

Resolve builtins inline during `advance`. When advance encounters `FlatAction::Invoke` with a `HandlerKind::Builtin` handler, it calls `execute_builtin` directly and delivers the result to the parent via `deliver()`. No Invoke frame is created, no TaskId is assigned, no DispatchEvent is pushed. Builtins become invisible to the event loop.

### advance: Invoke arm

```rust
// barnum_engine/src/advance.rs — Invoke arm (before, lines 35-50)
FlatAction::Invoke { handler } => {
    let task_id = workflow_state.next_task_id();
    let frame_id = workflow_state.insert_frame(Frame {
        parent,
        kind: FrameKind::Invoke { handler },
    });
    workflow_state.task_to_frame.insert(task_id, frame_id);
    workflow_state.pending_effects.push_back((
        frame_id,
        PendingEffectKind::Dispatch(DispatchEvent {
            task_id,
            handler_id: handler,
            value,
        }),
    ));
}

// barnum_engine/src/advance.rs — Invoke arm (after)
FlatAction::Invoke { handler } => {
    match workflow_state.flat_config.handler(handler) {
        HandlerKind::Builtin(builtin_handler) => {
            let result = barnum_builtins::execute_builtin(
                &builtin_handler.builtin,
                &value,
            )?;
            return super::complete::deliver(workflow_state, parent, result);
        }
        HandlerKind::TypeScript(_) => {
            let task_id = workflow_state.next_task_id();
            let frame_id = workflow_state.insert_frame(Frame {
                parent,
                kind: FrameKind::Invoke { handler },
            });
            workflow_state.task_to_frame.insert(task_id, frame_id);
            workflow_state.pending_effects.push_back((
                frame_id,
                PendingEffectKind::Dispatch(DispatchEvent {
                    task_id,
                    handler_id: handler,
                    value,
                }),
            ));
        }
    }
}
```

The Builtin arm calls `execute_builtin` and returns the result of `deliver()`. The TypeScript arm is the existing code, unchanged.

### advance return type

`advance` currently returns `Result<(), AdvanceError>`. It must now propagate two things it couldn't before:

1. **Terminal values.** When a workflow is entirely builtins (e.g., `Chain(Constant(1), Identity)`), advance fully resolves it. `deliver` with `parent: None` returns `Ok(Some(value))`. The caller needs to see this.

2. **Deliver errors.** `deliver` returns `Result<Option<Value>, CompleteError>`. This includes `CompleteError::InvalidHandlerOutput` (when a ResumePerform handler returns a value that doesn't destructure as `[value, state]`). A builtin in a resume handler's action tree can trigger this path.

Change advance's return type to `Result<Option<Value>, CompleteError>`:

```rust
// barnum_engine/src/advance.rs — advance signature (before)
pub fn advance(
    workflow_state: &mut WorkflowState,
    action_id: ActionId,
    value: Value,
    parent: Option<ParentRef>,
) -> Result<(), AdvanceError> {

// barnum_engine/src/advance.rs — advance signature (after)
pub fn advance(
    workflow_state: &mut WorkflowState,
    action_id: ActionId,
    value: Value,
    parent: Option<ParentRef>,
) -> Result<Option<Value>, CompleteError> {
```

`CompleteError` already wraps `AdvanceError` via `#[from]`, so existing `?` on advance-error-producing calls still works. Non-terminal branches return `Ok(None)` instead of `Ok(())`.

### AdvanceError: add Builtin variant

`execute_builtin` returns `Result<Value, BuiltinError>`. Propagate this through AdvanceError:

```rust
// barnum_engine/src/lib.rs — AdvanceError (add variant)
pub enum AdvanceError {
    // ... existing variants unchanged ...

    /// A builtin handler produced a type mismatch error.
    #[error(transparent)]
    Builtin(#[from] barnum_builtins::BuiltinError),
}
```

The `?` in the Invoke arm converts `BuiltinError` into `AdvanceError::Builtin`, then the outer `?` converts `AdvanceError` into `CompleteError::Advance`.

### barnum_builtins: regular dependency

Move `barnum_builtins` from `[dev-dependencies]` to `[dependencies]` in barnum_engine's Cargo.toml. `barnum_builtins` depends only on `barnum_ast` and `serde_json`, both of which `barnum_engine` already depends on. No new transitive dependencies.

```toml
# barnum_engine/Cargo.toml (before)
[dependencies]
barnum_ast = { path = "../barnum_ast" }
# ...

[dev-dependencies]
barnum_builtins = { path = "../barnum_builtins" }
# ...

# barnum_engine/Cargo.toml (after)
[dependencies]
barnum_ast = { path = "../barnum_ast" }
barnum_builtins = { path = "../barnum_builtins" }
# ...

[dev-dependencies]
# barnum_builtins removed from here
# ...
```

### All and ForEach: propagate terminal advance

The for loop over children must check whether a child's advance terminated the workflow. This happens when all children are builtins (or when an earlier builtin fills the last remaining All/ForEach slot, triggering cascading deliver up to the root).

```rust
// barnum_engine/src/advance.rs — All for loop (before, lines 86-97)
for (i, child) in children.into_iter().enumerate() {
    advance(
        workflow_state,
        child,
        value.clone(),
        Some(ParentRef::All {
            frame_id,
            child_index: i,
        }),
    )?;
}

// barnum_engine/src/advance.rs — All for loop (after)
for (i, child) in children.into_iter().enumerate() {
    if let Some(terminal_value) = advance(
        workflow_state,
        child,
        value.clone(),
        Some(ParentRef::All {
            frame_id,
            child_index: i,
        }),
    )? {
        return Ok(Some(terminal_value));
    }
}
```

Same pattern for ForEach (lines 118-128).

### Fix existing vacuous deliver calls

advance already calls `deliver` for vacuous All (0 children) and ForEach (empty array) at lines 69-71 and 108-110. These currently use `.expect()` and discard the return value:

```rust
// barnum_engine/src/advance.rs — vacuous All (before, lines 69-71)
super::complete::deliver(workflow_state, parent, Value::Array(vec![]))
    .expect("vacuous empty-parallel completion should not fail");
return Ok(());

// barnum_engine/src/advance.rs — vacuous All (after)
return super::complete::deliver(workflow_state, parent, Value::Array(vec![]));
```

Same for ForEach. The `.expect()` and `Ok(())` are replaced by returning deliver's result directly. This fixes a latent bug: `All([])` as the root workflow would have panicked on the `.expect()` or silently lost the terminal value.

### Chain arm: propagate terminal advance

```rust
// barnum_engine/src/advance.rs — Chain arm (before, lines 52-64)
FlatAction::Chain { rest } => {
    let first = workflow_state.flat_config.chain_first(action_id);
    let frame_id = workflow_state.insert_frame(Frame {
        parent,
        kind: FrameKind::Chain { rest },
    });
    advance(
        workflow_state,
        first,
        value,
        Some(ParentRef::Chain { frame_id }),
    )?;
}

// barnum_engine/src/advance.rs — Chain arm (after)
FlatAction::Chain { rest } => {
    let first = workflow_state.flat_config.chain_first(action_id);
    let frame_id = workflow_state.insert_frame(Frame {
        parent,
        kind: FrameKind::Chain { rest },
    });
    if let Some(terminal_value) = advance(
        workflow_state,
        first,
        value,
        Some(ParentRef::Chain { frame_id }),
    )? {
        return Ok(Some(terminal_value));
    }
}
```

When `Chain(Builtin, Rest)` is advanced, the builtin resolves inline, delivers to the Chain frame, Chain trampolines to Rest. If Rest also resolves (another builtin or a cascading completion), the terminal value propagates up through deliver. The `if let Some` catches it.

The same pattern applies to ResumeHandle (body advance at line 160-165) and RestartHandle (body advance at lines 185-193).

### deliver: Chain trampoline already works

`deliver`'s Chain arm calls `advance(workflow_state, rest, value, frame.parent)?; Ok(None)` (complete.rs:77-78). With the return type change, advance can return `Some(terminal_value)`. The Chain arm must propagate it:

```rust
// barnum_engine/src/complete.rs — deliver Chain arm (before, lines 66-78)
ParentRef::Chain { frame_id } => {
    let frame = workflow_state
        .frames
        .remove(frame_id)
        .expect("parent frame exists");
    let FrameKind::Chain { rest } = frame.kind else { unreachable!(...) };
    super::advance::advance(workflow_state, rest, value, frame.parent)?;
    Ok(None)
}

// barnum_engine/src/complete.rs — deliver Chain arm (after)
ParentRef::Chain { frame_id } => {
    let frame = workflow_state
        .frames
        .remove(frame_id)
        .expect("parent frame exists");
    let FrameKind::Chain { rest } = frame.kind else { unreachable!(...) };
    super::advance::advance(workflow_state, rest, value, frame.parent)
}
```

`advance` returns `Result<Option<Value>, CompleteError>`, which is exactly what `deliver` returns. The explicit `Ok(None)` is replaced by returning advance's result directly.

Same change for the RestartHandle handler-complete arm (complete.rs:139-148).

### Event loop: initial advance handles termination

```rust
// barnum_event_loop/src/lib.rs — run_workflow (before, lines 300-301)
let root = workflow_state.workflow_root();
advance(workflow_state, root, Value::Null, None).expect("initial advance failed");

// barnum_event_loop/src/lib.rs — run_workflow (after)
let root = workflow_state.workflow_root();
if let Some(terminal_value) = advance(workflow_state, root, Value::Null, None)? {
    return Ok(terminal_value);
}
```

A workflow of only builtins terminates during the initial advance.

### Event loop: completion advance handles termination

After `complete()` returns `None` (workflow still running), the completion path in deliver may have called advance for Chain trampolines, which may have resolved builtins. Currently this is invisible because advance returns `()`. With the return type change, `complete()` already returns `Option<Value>` — the terminal value propagates through deliver's recursive calls, through complete, and out to the event loop. No change needed here; the existing `if let Some(terminal_value) = complete(...)` handles it.

### Scheduler: remove Builtin arm

The Scheduler no longer receives builtin dispatches. Remove the `HandlerKind::Builtin` match arm from `Scheduler::dispatch`:

```rust
// barnum_event_loop/src/lib.rs — Scheduler::dispatch (before, lines 102-131)
pub fn dispatch(&self, dispatch_event: &DispatchEvent, handler: &HandlerKind) {
    let result_tx = self.result_tx.clone();
    let task_id = dispatch_event.task_id;

    match handler {
        HandlerKind::Builtin(builtin_handler) => {
            let builtin_kind = builtin_handler.builtin.clone();
            let value = dispatch_event.value.clone();
            tokio::spawn(async move {
                let result = execute_builtin(&builtin_kind, &value).map_err(HandlerError::from);
                let _ = result_tx.send((task_id, result));
            });
        }
        HandlerKind::TypeScript(ts) => {
            // ... subprocess execution ...
        }
    }
}

// barnum_event_loop/src/lib.rs — Scheduler::dispatch (after)
pub fn dispatch(&self, dispatch_event: &DispatchEvent, handler: &HandlerKind) {
    let result_tx = self.result_tx.clone();
    let task_id = dispatch_event.task_id;

    let HandlerKind::TypeScript(ts) = handler else {
        panic!("Scheduler::dispatch called with non-TypeScript handler");
    };

    let module = ts.module.lookup().to_owned();
    let func = ts.func.lookup().to_owned();
    let value = dispatch_event.value.clone();
    let executor = self.executor.clone();
    let worker_path = self.worker_path.clone();

    tokio::spawn(async move {
        let result =
            execute_typescript(&executor, &worker_path, &module, &func, &value)
                .await
                .map_err(HandlerError::from);
        let _ = result_tx.send((task_id, result));
    });
}
```

The `barnum_builtins` dependency can be removed from `barnum_event_loop` since the event loop no longer calls `execute_builtin`. `HandlerError::Builtin` becomes dead code and is removed.

### Event loop: dispatch arm simplifies

The Dispatch arm no longer needs to look up the handler kind — all dispatches are TypeScript:

```rust
// barnum_event_loop/src/lib.rs — Dispatch arm (before, lines 329-341)
EventKind::Dispatch(dispatch_event) => {
    validate_value(
        &compiled_schemas.input,
        dispatch_event.handler_id,
        &dispatch_event.value,
        SchemaDirection::Input,
        workflow_state,
    )?;

    let handler = workflow_state.handler(dispatch_event.handler_id);
    scheduler.dispatch(&dispatch_event, handler);
}

// barnum_event_loop/src/lib.rs — Dispatch arm (after)
EventKind::Dispatch(dispatch_event) => {
    validate_value(
        &compiled_schemas.input,
        dispatch_event.handler_id,
        &dispatch_event.value,
        SchemaDirection::Input,
        workflow_state,
    )?;

    let handler = workflow_state.handler(dispatch_event.handler_id);
    scheduler.dispatch(&dispatch_event, handler);
}
```

The Dispatch arm itself is unchanged. The simplification is that `scheduler.dispatch` no longer branches on handler kind internally.

### Test helpers: delete drive_builtins and complete_and_drive

After advance resolves builtins inline, the pending effects queue contains only TypeScript dispatches and restart events. `drive_builtins` is deleted. `complete_and_drive` is deleted.

Tests that called `drive_builtins` after the initial advance now pop TypeScript dispatches directly:

```rust
// Before
let root = engine.workflow_root();
advance(&mut engine, root, json!({"input": 1}), None).unwrap();
let (result, ts) = drive_builtins(&mut engine).unwrap();
assert_eq!(ts.len(), 2);

// After
let root = engine.workflow_root();
let result = advance(&mut engine, root, json!({"input": 1}), None).unwrap();
let d0 = pop_dispatch(&mut engine).unwrap();
let d1 = pop_dispatch(&mut engine).unwrap();
assert!(pop_dispatch(&mut engine).is_none());
```

Tests that called `complete_and_drive` call `complete` directly:

```rust
// Before
let (result, ts) = complete_and_drive(
    &mut engine,
    CompletionEvent { task_id: d.task_id, value: json!("out") },
).unwrap();

// After
let result = complete(&mut engine, CompletionEvent {
    task_id: d.task_id,
    value: json!("out"),
}).unwrap();
// pop any newly produced TypeScript dispatches if needed
```

### Restart effects: still in the pending queue

Restart events (`PendingEffectKind::Restart`) are deferred effects, not builtins. They stay in the pending effects queue and are processed by the event loop (production) or by test code that pops and calls `process_restart`. No change to restart processing.

Tests that relied on `drive_builtins` to process both builtins AND restarts need to handle restarts explicitly. A small helper like `process_pending_restarts` may be useful:

```rust
// barnum_engine/src/test_helpers.rs — new helper
pub fn process_pending_restarts(
    engine: &mut WorkflowState,
) -> Result<Option<Value>, CompleteError> {
    loop {
        let Some((frame_id, pending_effect_kind)) = engine.pop_pending_effect() else {
            return Ok(None);
        };
        if !engine.is_frame_live(frame_id) {
            continue;
        }
        match pending_effect_kind {
            PendingEffectKind::Restart(restart_event) => {
                process_restart(engine, restart_event)?;
            }
            PendingEffectKind::Dispatch(_) => {
                // Put it back — this is a TypeScript dispatch, not a restart.
                engine.pending_effects.push_front((frame_id, pending_effect_kind));
                return Ok(None);
            }
        }
    }
}
```

This processes all restart events at the front of the queue until it hits a TypeScript dispatch or the queue is empty. Restart processing calls advance, which resolves any resulting builtins inline, so only TypeScript dispatches and restart events appear in the queue afterward.

## What changes

| Component | Before | After |
|-----------|--------|-------|
| `advance` Invoke arm | Creates Invoke frame, TaskId, DispatchEvent for all handlers | Builtins: `execute_builtin` + `deliver` inline. TypeScript: unchanged. |
| `advance` return type | `Result<(), AdvanceError>` | `Result<Option<Value>, CompleteError>` |
| `AdvanceError` | No builtin variant | `Builtin(BuiltinError)` variant added |
| `barnum_builtins` dep | dev-dependency of `barnum_engine` | regular dependency of `barnum_engine` |
| All/ForEach for loop | Ignores advance return | Propagates `Some(terminal_value)` |
| Vacuous All/ForEach | `.expect()` on deliver, discards result | Returns deliver result directly |
| `deliver` Chain arm | `advance(...)?; Ok(None)` | `advance(...)` (returns its result) |
| `Scheduler::dispatch` | Matches on Builtin/TypeScript | TypeScript only; panics on Builtin |
| `HandlerError::Builtin` | Exists | Removed (dead code) |
| `barnum_event_loop` dep on `barnum_builtins` | Yes | Removed |
| Event loop initial advance | `.expect("initial advance failed")` | `if let Some(terminal_value) = advance(...)? { return Ok(terminal_value); }` |
| `drive_builtins` | Test helper that processes builtins + restarts | Deleted |
| `complete_and_drive` | Test helper that completes + drives builtins | Deleted |
| Test patterns | `advance; drive_builtins; complete_and_drive` | `advance; pop_dispatch; complete` |

## Open questions

1. **Recursion depth for builtin chains.** `Chain(B1, Chain(B2, Chain(B3, ...)))` where each Bi is a builtin creates O(n) recursion depth in advance (each builtin resolves, delivers to Chain, trampolines to the next). This mirrors the existing recursion in `deliver` for Chain trampolines. In practice, workflows don't chain thousands of builtins, so this is fine. If it ever becomes a problem, advance could be rewritten as an iterative trampoline.

2. **Should `Scheduler::dispatch` panic on builtins or take `&TypeScriptHandler` directly?** The panic-on-unexpected-variant approach preserves the current signature. Narrowing the signature to take `&TypeScriptHandler` would be cleaner (impossible states are unrepresentable) but changes the call site in the event loop. The refactor doc shows the panic approach for simplicity; narrowing the signature is a clean follow-up.
