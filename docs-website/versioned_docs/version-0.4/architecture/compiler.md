# Compiler and Execution Model

The Rust side of Barnum has two phases: **compilation** (flatten the tree AST into a linear array) and **execution** (drive the state machine until the workflow terminates). This page covers both.

## Compilation: tree → FlatConfig

The JSON AST is a tree. Trees are cache-unfriendly — pointer chasing through heap-allocated nodes. The compiler flattens the tree into a `FlatConfig`: a contiguous `Vec<FlatEntry>` where every cross-reference is an index, not a pointer.

### The FlatConfig structure

```rust
struct FlatConfig {
    entries: Vec<FlatEntry>,       // Linear array of 8-byte entries
    handlers: Vec<HandlerKind>,    // Handler pool (interned)
    workflow_root: ActionId,       // Entry point
}
```

`FlatEntry` is an enum that fits in exactly 8 bytes (verified by a compile-time assertion):

```rust
enum FlatEntry {
    Action(FlatAction),               // An executable action
    ChildRef { action: ActionId },    // Pointer to a multi-entry child elsewhere
    BranchKey { key: KindDiscriminator }, // Branch case discriminant
}
```

`FlatAction` mirrors the nine AST node types but with index-based references instead of pointers:

```rust
enum FlatAction {
    Invoke { handler: HandlerId },
    Chain { rest: ActionId },
    All { count: Count },
    ForEach { body: ActionId },
    Branch { count: Count },
    ResumeHandle { resume_handler_id: ResumeHandlerId },
    ResumePerform { resume_handler_id: ResumeHandlerId },
    RestartHandle { restart_handler_id: RestartHandlerId },
    RestartPerform { restart_handler_id: RestartHandlerId },
}
```

### Layout rules

Each action type has a fixed layout in the entry array:

**Single-entry actions** (`Invoke`, `ForEach`, `ResumePerform`, `RestartPerform`) — occupy exactly one slot.

**Chain** — occupies one slot. The `first` subtree is flattened *immediately after* the Chain entry (at `action_id + 1`), so no child slot is needed. The `rest` ActionId is stored in the Chain entry itself.

```
0: Chain { rest: 2 }
1: Invoke(handler_A)     ← first (always at action_id + 1)
2: Invoke(handler_B)     ← rest
```

**All** — one slot plus `count` child slots. Each child slot either contains an inlined single-entry action or a `ChildRef` pointing to a multi-entry child flattened elsewhere.

```
0: All { count: 3 }
1: Invoke(handler_A)     ← inlined (single-entry)
2: ChildRef { action: 4 } ← pointer (multi-entry child)
3: Invoke(handler_C)     ← inlined
4: Chain { rest: 6 }     ← the multi-entry child
5: Invoke(handler_D)
6: Invoke(handler_E)
```

**Branch** — one slot plus `2 * count` entries: alternating `BranchKey` and child slots. Keys are sorted lexicographically for determinism.

```
0: Branch { count: 2 }
1: BranchKey { key: "Err" }
2: Invoke(handler_err)       ← child slot (inlined)
3: BranchKey { key: "Ok" }
4: ChildRef { action: 5 }   ← child slot (pointer)
5: Chain { rest: 7 }         ← the Ok handler
6: Invoke(handler_a)
7: Invoke(handler_b)
```

**ResumeHandle / RestartHandle** — three slots: the parent entry plus child slots for body (`action_id + 1`) and handler (`action_id + 2`).

### Handler interning

Identical handlers share a single `HandlerId`:

```rust
fn intern_handler(&mut self, handler: HandlerKind) -> HandlerId {
    if let Some(index) = self.handlers.iter().position(|h| h == &handler) {
        return HandlerId(index as u32);
    }
    let index = self.handlers.len();
    self.handlers.push(handler);
    HandlerId(index as u32)
}
```

In `pipe(A, A, B)`, both `A` invocations share `HandlerId(0)`. This matters for schema validation: a handler's input/output validators are compiled once per `HandlerId`, not once per invocation site.

### The flattening algorithm

The flattener uses a **pre-allocate + fill** strategy:

1. **`flatten_action(action)`** — allocate one slot, write the action into it, return its `ActionId`.
2. **`fill_child_slot(action, slot)`** — if the child is single-entry, inline it directly into the pre-allocated slot. If multi-entry, flatten it elsewhere via `flatten_action` and write a `ChildRef` into the slot.
3. For `Chain`, the `first` subtree is flattened immediately after allocation (guaranteed adjacent by the `debug_assert`). The `rest` subtree is flattened after `first` completes.

The builder type `UnresolvedFlatConfig` holds `Vec<Option<FlatEntry>>` — slots are `None` until filled. `finalize()` verifies every slot was filled and produces the final `FlatConfig`.

---

## Execution: the event loop

The Rust runtime is an **event-driven state machine**. The engine itself (`barnum_engine`) is pure — no I/O, no async, no timers. The event loop (`barnum_event_loop`) provides the async runtime and subprocess scheduling.

### WorkflowState

```rust
struct WorkflowState {
    flat_config: FlatConfig,
    frames: Arena<Frame>,                      // Frame tree (runtime stack)
    task_to_frame: BTreeMap<TaskId, FrameId>,  // In-flight handler → frame
    pending_effects: VecDeque<PendingEffect>,  // Work queue
    next_task_id: u32,
}
```

The frame arena uses `thunderdome::Arena`, which provides generational indices. A `FrameId` from a removed frame will not resolve even if the slot is reused — this is the foundation of the liveness check.

### Frames

Frames are the runtime stack. Each frame tracks the state of an active computation:

```rust
enum FrameKind {
    Chain { rest: ActionId },                    // Waiting for first to complete
    All { results: Vec<Option<Value>> },        // Collecting N concurrent results
    ForEach { results: Vec<Option<Value>> },    // Collecting per-element results
    Invoke { handler: HandlerId },              // Handler in flight
    ResumeHandle(ResumeHandleFrame),            // Effect handler with state
    ResumePerform(ResumePerformFrame),          // Active resume handler invocation
    RestartHandle(RestartHandleFrame),          // Effect handler with body/handler
    RestartPerformMarker,                       // Liveness marker for deferred restarts
}
```

Frames reference their parent via `ParentRef`, which encodes both the parent's `FrameId` and the relationship type (Chain, All with child index, ForEach with child index, etc.). This eliminates a second dispatch in the completion path — `deliver()` matches on `ParentRef` directly.

### advance(): expansion

`advance()` takes an `ActionId` and a value, creates the appropriate frame, and either recurses (for structural actions) or enqueues work (for leaves):

- **Invoke**: creates an `Invoke` frame, enqueues a `Dispatch` effect.
- **Chain**: creates a `Chain` frame, advances `first` as its child.
- **All**: creates an `All` frame with `N` empty result slots, advances all children concurrently with the same input (cloned).
- **ForEach**: creates a `ForEach` frame, destructures the input array, advances each element through the body.
- **Branch**: reads `value["kind"]`, finds the matching case, advances it. No frame — Branch is transparent.
- **RestartPerform**: walks the ancestor chain to find the matching `RestartHandle`, creates a `RestartPerformMarker` frame, enqueues a deferred `Restart` effect.

### complete(): delivery

When a handler finishes, `complete()` removes the `Invoke` frame and delivers the result to the parent:

- **Chain parent**: removes the Chain frame, calls `advance()` on `rest` with the result. This is the **trampoline** — Chain doesn't recurse; it tail-calls via advance.
- **All/ForEach parent**: stores the result in the indexed slot. When all slots are filled, collects them into a `Value::Array` and delivers to the grandparent.
- **RestartHandle parent (body side)**: body completed normally — removes the frame, delivers to the grandparent.
- **RestartHandle parent (handler side)**: handler completed — re-advances the body with the handler's output as the new input.
- **ResumeHandle parent**: body completed — removes the frame, delivers to the grandparent.
- **ResumePerform parent**: handler completed — destructures `[value, new_state]`, writes `new_state` back to the `ResumeHandle` frame, delivers `value` upward.
- **No parent**: workflow complete — return the terminal value.

### The event loop

The event loop in `run_workflow` ties everything together:

```rust
loop {
    // 1. Pop pending effects (dispatch or restart)
    if let Some((frame_id, effect)) = workflow_state.pop_pending_effect() {
        if !workflow_state.is_frame_live(frame_id) { continue; }
        match effect {
            Dispatch(event) => {
                validate_input(event);
                scheduler.dispatch(event, handler);
            }
            Restart(event) => {
                process_restart(workflow_state, event);
            }
        }
    }
    // 2. If no pending effects, wait for a handler completion
    else {
        let (task_id, value) = scheduler.recv().await;
        if !workflow_state.is_task_live(task_id) { continue; }
        validate_output(value);
        if let Some(terminal) = complete(workflow_state, completion) {
            return terminal;
        }
    }
}
```

Key properties:

- **Pending effects are drained first.** After each `advance()` or `complete()` call, new effects may be enqueued. The loop drains them before waiting for external completions. This ensures restarts are processed immediately.
- **Liveness checks prevent use-after-free.** When a `RestartHandle` tears down its body, all descendant frames are removed. In-flight tasks whose frames were removed are silently dropped when their completions arrive — the `FrameId` no longer resolves.
- **Builtins execute in-process.** The scheduler spawns tokio tasks for both builtins (inline `execute_builtin`) and TypeScript handlers (subprocess `execute_typescript`). Builtins complete immediately; TypeScript handlers await subprocess I/O.

### Subprocess execution

Each TypeScript handler invocation spawns an isolated subprocess:

```
executor worker.ts module func
```

The subprocess receives `{ "value": <input> }` on stdin and writes the JSON result to stdout. The Rust side deserializes the output and feeds it back as a completion. Each handler runs in complete isolation — it cannot see other handlers' context, state, or even know what step comes next.
