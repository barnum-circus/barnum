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
            self.stashed_items.push(StashedItem::Effect { ... });
            return Ok(StashOutcome::Stashed);
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

`deliver_or_stash` and `find_blocking_ancestor` would also use `parent_iter`, keeping the walk logic in one place. The iterator handles the "frame gone" case by terminating early (the caller checks whether it ended due to a missing frame or reaching the root).

Not worth building until the parent walk logic is needed in more places or the fused single-pass matters for performance.
