# Deferred: Provider, Capabilities, State Monad, Coroutine RPC

These are extensions to the Handle/Perform architecture that are NOT part of the current implementation plan. Captured here so the ideas aren't lost and so the current design doesn't accidentally preclude them.

None of this should be built until Phases 1-6 are complete and stable.

## Provider (Handle that always Resumes)

A Handle whose handler DAG always produces `{ kind: "Resume", value }`. It intercepts a request effect and provides a dynamically computed value back to the continuation. The continuation resumes as if the Perform returned the provided value.

`declare` is a degenerate provider — the value is pre-computed and stored in the Handle frame's bindings. A general `provider` would compute the value from the payload at interception time.

```ts
provider(
  (request) => pipe(
    fetchFromApi,
    request,       // Perform — suspends, handler computes a value, continuation resumes with it
    useResult,     // runs with the provided value
  ),
  (payload) => computeResponse(payload),  // handler DAG → { kind: "Resume", value }
)
```

Use cases:
- **Fallback/default values**: Catch a "might fail" signal, resume with a fallback instead of discarding the continuation (unlike tryCatch which discards).
- **Dependency injection**: Body requests a service; the Handle provides a concrete implementation. Like React's Context.Provider.
- **Dynamic configuration**: Body requests a config value; the Handle computes it based on environment, feature flags, etc.

No new substrate work needed — this is just a Handle whose handler DAG produces Resume. The only question is whether a named surface combinator (`provider`) adds enough clarity over writing the Handle directly.

## Capabilities (Object-Capability Security)

Handlers are opaque and sandboxed. If a handler needs to perform I/O (write a file, make a network request, mutate external state), it can't do so directly in a restricted environment. Instead, it returns an intent. The AST interprets the intent as a Perform. A Handle block acts as a capability grant — if no Handle intercepts the effect, it's an unhandled effect error. The handler is mathematically prevented from accessing the capability.

```ts
// The capability boundary:
withFileSystem("/tmp/sandbox", body)

// Compiles to Handle that intercepts fs:write effects,
// routes them to a trusted handler that enforces the sandbox root,
// and resumes the continuation with the result.
```

This models Object-Capability (OCap) security: capabilities are granted by lexical scope (the Handle block), not by ambient authority (global imports).

## State Monad (Get/Put Effects)

Mutable state spanning multiple handler invocations within a workflow. Two effects: Get (read state) and Put (write state). The Handle frame holds the state value. The handler DAGs for Get/Put read/write it and resume.

```ts
withState(initialValue, body)

// Body can contain Perform("state:get") and Perform("state:put")
// Handle frame maintains the current value
```

Requires the Handle frame to hold mutable local state (an `Option<Value>` or similar). The current Handle frame design (immutable bindings) would need extension.

## Coroutine RPC (Multi-step Handler Effects)

The fundamental tension: if a handler returns an intent (Free Monad / graph-level slicing), its call stack is destroyed. It can't receive the result and continue. The user must split logic across multiple handlers and stitch them in the AST.

For I/O-heavy handlers that need multiple capability accesses, this is unergonomic. The alternative is Coroutine RPC: the handler suspends (via async/await), sends an IPC message to the scheduler, and waits for a response.

### The IPC protocol extension

Currently the worker protocol has two message types:
- `Complete(task_id, result)` — handler finished
- (Error variant)

Coroutine RPC adds:
- `Yield(task_id, effect, payload)` — handler paused, requesting an effect
- `Resume(task_id, value)` — scheduler resolved the effect, handler can continue

### How it works

1. Handler calls `await ctx.perform("fs:write", { path, content })`.
2. The SDK sends `Yield(task_id, "fs:write", payload)` over IPC.
3. Tokio receives Yield. The Rust engine treats the Invoke frame like a Perform: severs it, assigns cont_id, bubbles the effect to the nearest Handle.
4. The Handle routes to a trusted capability handler. The capability handler writes the file.
5. The capability handler completes. The engine reconnects the Invoke frame and sends `Resume(task_id, result)` over IPC.
6. The SDK resolves the Promise. The handler's `await` unblocks. Execution continues.

The handler looks like normal procedural code. The scheduler maintains full control. This is the Temporal / Azure Durable Functions / AWS Step Functions architecture.

### Why it's deferred

- The current IPC protocol is simple (dispatch + complete). Adding Yield/Resume is a protocol change.
- The worker SDK doesn't exist. Handlers are currently plain functions.
- Graph-level slicing (Architecture 1) works for the current use cases. Handlers return unions, the AST branches.
- Coroutine RPC is needed when handlers are I/O-heavy and need multiple capability accesses mid-execution. Current demos don't require this.

### Design constraint for current work

The current Handle/Perform/Resume design should NOT preclude Coroutine RPC. Specifically: the Invoke frame's interaction with bubble_effect should be designed so that a future Yield message can trigger the same bubble_effect path that a Perform node triggers. The mechanism is the same — only the entry point differs (AST node vs IPC message).

## Parent Chain Iterator

`bubble_effect` currently walks the parent chain twice: once via `find_blocking_ancestor` (is this event blocked or targeting a gone frame?) and once via `find_and_dispatch_handler` (find the matching Handle). Both are O(depth) and the depth is bounded by nesting level, so the constant factor is negligible — but the duplication is aesthetically unsatisfying.

A `parent_iter(frame_id)` method returning an iterator over `(ParentRef, &Frame)` pairs would let both operations compose via iterator combinators:

```rust
/// Iterator over (parent_ref, &frame) pairs walking from a frame to the root.
fn parent_iter(&self, frame_id: FrameId) -> impl Iterator<Item = (ParentRef, &Frame)> { ... }

fn bubble_effect(...) -> Result<StashOutcome, AdvanceError> {
    let mut parents = self.parent_iter(starting_parent.frame_id());

    // Single pass: check blocking at each step, stop at matching Handle.
    // try_for_each / find_map style — short-circuits on block or match.
    for (parent_ref, frame) in &mut parents {
        if Self::is_blocked_by_handle(parent_ref, &frame.kind) {
            return Ok(StashOutcome::Blocked);
        }
        if let FrameKind::Handle(h) = &frame.kind {
            if h.effect_id == effect_id {
                // dispatch...
                return Ok(StashOutcome::Consumed);
            }
        }
    }
    // Iterator ended without yielding (frame gone) or without match.
    if parents.hit_gone_frame() {
        return Ok(StashOutcome::Consumed);
    }
    Err(AdvanceError::UnhandledEffect { effect_id })
}
```

`try_deliver` and `find_blocking_ancestor` would also use `parent_iter`, keeping the walk logic in one place. The iterator handles the "frame gone" case by terminating early (the caller checks whether it ended due to a missing frame or reaching the root).

Not worth building until the parent walk logic is needed in more places or the fused single-pass matters for performance.

## Typed Internal Data Representation

The engine currently uses `serde_json::Value` uniformly for all data flowing through the DAG. When `dispatch_to_handler` constructs `json!({"payload": payload, "state": state})` and the handler is entirely builtins (Constant, ExtractField, Tag), the data never leaves Rust — building a `serde_json::Value` just to destructure it in the next builtin is pure overhead.

Possible approaches:

1. **Two advance paths** — one for `Value` (external handlers at IPC boundaries), one for a typed internal representation. Doubles the surface area.
2. **Unified enum** — `enum FlowData { Json(Value), Structured(InternalRepr) }`. Every builtin handler handles both variants. Structured data is lazily serialized to JSON only at IPC boundaries.
3. **Zero-copy arena** — allocate handler inputs in a bump arena, pass references. Avoid `serde_json::Value` allocation entirely for internal flows.

Not worth pursuing until profiling shows JSON construction is a bottleneck. The current `Value`-everywhere design keeps Phase 1 simple and consistent with the existing engine.

## Teardown Optimization

`teardown_body` is O(n × depth): it iterates every frame in the arena and walks each one's parent chain to check `is_descendant_of`. For a slab with n frames and tree depth d, this is O(n × d) per teardown. The `task_to_parent` cleanup adds another O(t) pass where t is the number of in-flight tasks, but t ≤ n so the frame walk dominates.

Phase 1 accepts this cost — the arena is small during early use, and teardown only runs on Discard/RestartBody. But if workflows grow large (deep nesting, many concurrent branches), this becomes the bottleneck for handler completion.

### Approach 1: Child pointers

Each frame maintains a `Vec<FrameId>` of its direct children. `insert_frame` appends to the parent's child list. `teardown_body` walks the subtree top-down (BFS or DFS from the Handle's body child), collecting descendants in O(k) where k is the subtree size, not the full arena size.

Trade-offs:
- O(k) teardown instead of O(n × d). k ≤ n, and for a Handle whose body is one branch of a large All, k ≪ n.
- Child list maintenance cost: one `Vec::push` per `insert_frame`, one `Vec::retain` or swap-remove per `frames.remove`. Both O(1) amortized.
- Memory: one `Vec<FrameId>` per frame. For frames with few children (Chain has 1, Loop has 1), this is a 24-byte overhead (Vec's stack allocation) plus heap allocation on first push.
- `SmallVec<[FrameId; 2]>` would inline up to 2 children (covers Chain, Loop, Handle) and only heap-allocate for All/ForEach with many children.

### Approach 2: Subtree generation counter

Each Handle frame holds a monotonic generation counter. When `teardown_body` runs, it bumps the counter. Every frame stores the generation it was created under. Frames whose generation doesn't match the current Handle generation are considered dead — `complete_task` and `sweep_stash` check the generation before delivering.

Trade-offs:
- O(1) teardown — just bump a counter. Dead frames are lazily collected.
- Lazy collection means dead frames occupy arena slots until reclaimed. Needs a periodic GC pass or piggyback on sweep_stash.
- Doesn't handle nested Handles cleanly — a nested Handle's generation is independent of its ancestor's. Teardown of an outer Handle should invalidate all inner frames, but inner frames check their own Handle's generation, not the outer one.
- Complexity: generation tracking interacts with the generational arena's own generation counters, creating two levels of generation. Confusing.

### Approach 3: Parent-pointer walk with early termination

Keep the current parent-pointer-only structure but optimize `is_descendant_of` with a depth field. Each frame stores its depth (parent's depth + 1). `teardown_body` collects all frames with depth > Handle's depth whose parent chain passes through the Handle. The depth check prunes frames in unrelated subtrees early.

Trade-offs:
- Marginal improvement. Still O(n) iteration over the arena; the depth check only saves the parent walk for frames at shallower depths.
- Simple to implement — one `u32` field per frame.
- Not worth the complexity for a marginal constant-factor improvement.

### Recommendation

Approach 1 (child pointers) is the clear winner. O(k) teardown, simple implementation, no lazy-GC complexity. Use `SmallVec<[FrameId; 2]>` to avoid heap allocation for the common case (≤ 2 children). Defer until profiling shows teardown is a bottleneck or until workflows with large arenas become common.
