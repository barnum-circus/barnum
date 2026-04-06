# Algebraic Effect Handlers

Barnum's control flow combinators — `loop`, `tryCatch`, `earlyReturn`, `race`, `withTimeout` — are all implemented using the same substrate: **algebraic effect handlers**. This is not a metaphor. The AST has dedicated `Handle` and `Perform` nodes, the runtime has a frame-based handler stack, and effects propagate by walking the ancestor chain — the same architecture as a language designed around algebraic effects.

This design is strongly influenced by [Andrej Bauer](https://www.andrej.com/)'s work on algebraic effects and the [Eff programming language](https://www.eff-lang.org/). Bauer's [OPLSS 2018 lecture series](https://www.youtube.com/watch?v=atYp386EGo8) — *What's Algebraic About Algebraic Effects and Handlers?* — is the clearest exposition of the theory. Effects and handlers form an algebraic structure: operations (effects) are free and handlers give them meaning, separating the *description* of a side effect from its *interpretation*.

Barnum adapts this model for a workflow runtime with two specialized handler types instead of general-purpose delimited continuations.

## Two handler types

Traditional algebraic effects (as in Eff) have one handler model: an effect is raised, the handler captures the continuation, and the handler decides whether and how to resume it. Barnum splits this into two specialized variants:

### ResumeHandle (inline, state-preserving)

The handler runs **immediately at the Perform site** without suspending the body. It receives `[payload, state]` and returns `[value, new_state]`. The `value` is delivered to the Perform site's parent as if it were a normal completion. The `new_state` is written back to the ResumeHandle frame for the next invocation.

Used by: `bind` (concurrent variable capture).

```
Body:        ... → ResumePerform(id) → ...
                         ↓
Handler:     receives [payload, state]
             returns [value, new_state]
                         ↓
Body:        ... ← value delivered ← ...
```

The handler runs as a child DAG of the Perform frame. Multiple concurrent Performs can be in flight simultaneously — no serialization, no blocking.

### RestartHandle (teardown, re-execution)

When `RestartPerform` fires, the **entire body is torn down** — all descendant frames are removed from the arena, all in-flight tasks are orphaned. The handler then runs with `[payload, state]`. When the handler completes, its output becomes the **new body input**, and the body re-advances from scratch.

Used by: `loop`, `tryCatch`, `earlyReturn`, `race`, `withTimeout`.

```
Body:        ... → RestartPerform(id) → [body torn down]
                         ↓
Handler:     receives [payload, state]
             returns new_body_input
                         ↓
Body:        re-advances from scratch with new_body_input
```

This is a departure from traditional algebraic effects, where the continuation is captured and can be resumed multiple times. Barnum's restart semantics are closer to exception handling with restart — but with a crucial difference: the handler can provide a new input to the body, not just re-raise or recover.

## How loop compiles

`loop` is the canonical example. Here's how the TypeScript DSL compiles it:

```ts
loop<string>((recur, done) =>
  pipe(step, classify).branch({
    Continue: recur,    // restart the loop
    Break: done,        // exit the loop
  })
)
```

This desugars to:

```ts
Chain(
  Tag("Continue"),                        // tag input as Continue
  RestartHandle(id,
    Branch({
      Continue: Chain(ExtractField("value"), body),  // loop body
      Break: Chain(ExtractField("value"), Identity),  // exit path
    }),
    ExtractIndex(0),                      // handler: extract payload from [payload, state]
  )
)
```

The execution flow:

1. Input is tagged `{ kind: "Continue", value: input }`.
2. Branch takes the Continue arm, body executes.
3. If the body produces `{ kind: "Continue", value: next_input }` → `RestartPerform` fires → body torn down → handler extracts `next_input` → body re-advances → Branch takes Continue again.
4. If the body produces `{ kind: "Break", value: result }` → `RestartPerform` fires → body torn down → handler extracts `result` → body re-advances → Branch takes Break arm → Identity passes through → `RestartHandle` exits.

The `recur` and `done` tokens are `TypedAction` values, not functions. `recur` is `Chain(Tag("Continue"), RestartPerform(id))` — it tags the value and raises the effect. `done` is `Chain(Tag("Break"), RestartPerform(id))`.

## How tryCatch compiles

`tryCatch` uses the exact same `RestartHandle + Branch` substrate:

```ts
tryCatch(
  (throwError) => body,
  recovery
)
```

Compiles to:

```ts
Chain(
  Tag("Continue"),
  RestartHandle(id,
    Branch({
      Continue: Chain(ExtractField("value"), body),
      Break: Chain(ExtractField("value"), recovery),
    }),
    ExtractIndex(0),
  )
)
```

The only difference from `loop`: the Break arm runs `recovery` instead of `Identity`. When `throwError` fires, the body is torn down and the recovery handler runs with the error payload.

## How earlyReturn compiles

Same substrate, different semantics:

```ts
earlyReturn((exit) => body)
```

The body runs in the Continue arm. If `exit` fires, the Break arm runs `Identity` — the value passes through and exits the `RestartHandle`. Normal body completion also exits normally.

## How race compiles

`race` runs multiple actions concurrently and returns the first to complete:

```ts
race(a, b, c)
```

Compiles to:

```ts
Chain(
  Tag("Continue"),
  RestartHandle(id,
    Branch({
      Continue: All(
        Chain(a, Tag("Break"), RestartPerform(id)),
        Chain(b, Tag("Break"), RestartPerform(id)),
        Chain(c, Tag("Break"), RestartPerform(id)),
      ),
      Break: Identity,
    }),
    ExtractIndex(0),
  )
)
```

All three branches run concurrently inside `All`. The first to complete tags its result as Break and fires `RestartPerform`. The body (the `All` and all its children) is torn down — the other two branches' in-flight tasks are orphaned. The handler extracts the winner's payload, Branch takes the Break arm, and the result exits.

## Effect propagation

When `RestartPerform` fires during `advance()`, the runtime walks the frame ancestor chain to find the matching `RestartHandle`:

```rust
let restart_handle_frame_id =
    ancestors(&workflow_state.frames, starting_parent)
        .find_map(|(edge, frame)| {
            if let FrameKind::RestartHandle(h) = &frame.kind
                && h.restart_handler_id == restart_handler_id
            {
                Some(edge.frame_id())
            } else {
                None
            }
        })
        .ok_or(AdvanceError::UnhandledRestartEffect { .. })?;
```

This is the algebraic effects equivalent of stack unwinding — but instead of actually unwinding, a deferred `Restart` event is enqueued. The restart is processed later by the event loop, which tears down the body and advances the handler.

**Effect shadowing** works naturally: if an inner `RestartHandle` has the same `restart_handler_id` as an outer one, the inner handler intercepts the effect first. The ancestor walk stops at the first match.

## Deferred restarts and liveness

Restart effects are **not processed immediately**. When `RestartPerform` fires:

1. A `RestartPerformMarker` frame is created as a child of the Perform site.
2. A `PendingEffectKind::Restart` is enqueued in `pending_effects`.
3. `advance()` continues expanding the current action to completion.

The event loop processes the restart later:

1. Check liveness: is the marker frame still in the arena?
2. If live: tear down the body (remove all descendant frames), advance the handler.
3. If stale: skip (the body was already torn down by a prior restart or completion).

Why deferred? Consider `All(RestartPerform(id), invoke("b"))`. Both children advance during the same `advance()` call. The RestartPerform fires first, but `invoke("b")` also creates its frame and enqueues a dispatch. If the restart were processed immediately, the teardown would remove `b`'s frame before `advance()` finishes — a use-after-free. Deferring ensures `advance()` completes cleanly; the teardown happens in the event loop, and `b`'s stale dispatch is dropped by the liveness check.

## Body teardown

When a restart is processed, `teardown_body` removes every frame that is a descendant of the `RestartHandle`'s body side:

```rust
fn teardown_body(frames: &mut Arena<Frame>, task_to_frame: &mut BTreeMap<TaskId, FrameId>, restart_handle_frame_id: FrameId) {
    let to_remove: Vec<FrameId> = frames.iter()
        .filter_map(|(id, _)| {
            if is_descendant_of_body(frames, id, restart_handle_frame_id) {
                Some(id)
            } else {
                None
            }
        })
        .collect();

    for id in &to_remove {
        frames.remove(*id);
    }
    task_to_frame.retain(|_, frame_id| !to_remove.contains(frame_id));
}
```

After teardown:
- All body frames are gone. Any in-flight tasks for those frames will have stale `FrameId`s.
- When their completions arrive, `workflow_state.task_frame_id(task_id)` returns `None`, and the event loop skips them.
- The `RestartPerformMarker` (which was in the body subtree) is also removed, so any duplicate restart events for the same effect are silently dropped.

## Relationship to algebraic effects theory

Barnum's implementation maps to the theoretical framework as follows:

| Concept | Theory (Eff) | Barnum |
|---------|-------------|--------|
| **Effect** | `perform op v` | `RestartPerform(id)` / `ResumePerform(id)` |
| **Handler** | `handle ... with \| op v k → ...` | `RestartHandle` / `ResumeHandle` frame |
| **Continuation** | First-class `k` that can be called 0+ times | Implicit: restart re-advances body; resume delivers value upward |
| **Handler scope** | Lexical | Dynamic: ancestor chain walk |
| **Effect matching** | By operation name | By `restart_handler_id` / `resume_handler_id` |
| **Composability** | Handlers compose by nesting | Handlers compose by nesting — inner shadows outer |

Barnum doesn't need general-purpose multi-shot continuations. Workflow orchestration has two patterns — *retry from scratch* (restart) and *read a value* (resume) — and each gets a specialized implementation rather than being built on a general continuation mechanism.

## Further reading

- Andrej Bauer, [*What's Algebraic About Algebraic Effects and Handlers?*](https://www.youtube.com/watch?v=atYp386EGo8) (OPLSS 2018) — four-part lecture series covering the theoretical foundations
- Andrej Bauer and Matija Pretnar, [*Programming with Algebraic Effects and Handlers*](https://arxiv.org/abs/1203.1539) — the foundational paper behind Eff
- [Eff programming language](https://www.eff-lang.org/) — a functional language with first-class algebraic effects
