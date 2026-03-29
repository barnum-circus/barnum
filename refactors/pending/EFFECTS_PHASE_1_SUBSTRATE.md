# Phase 1: Effect Substrate

## Goal

Build the structural routing mechanism for algebraic effects in the Rust scheduler. After this phase, a Perform can suspend execution, bubble up to a Handle, and be resumed or discarded. No semantic effects yet — just the plumbing.

## Prerequisites

None. This is the foundation.

## New AST nodes (tree AST, TypeScript)

```ts
export interface HandleAction {
  kind: "Handle";
  handlers: Record<string, Action>;  // effect_type -> handler DAG
  body: Action;
}

export interface PerformAction {
  kind: "Perform";
  effect: string;  // routing key (matches a key in HandleAction.handlers)
}

export interface ResumeAction {
  kind: "Resume";
  // Input is { cont_id: number, value: Value }
  // The scheduler looks up cont_id and delivers value to the suspended continuation.
}
```

Handle maps effect type strings to handler DAGs. When a Perform fires with a matching effect type, the handler DAG is advanced with `{ payload: <pipeline_value>, cont_id: <token> }`.

Resume takes `{ cont_id, value }` from the pipeline and reconnects the suspended continuation, delivering `value` into it.

### Open question: enum vs string for effect types

The `effect` field on PerformAction and the keys in HandleAction.handlers could be:

- **Strings**: Open-ended, no Rust changes for new effects. But string matching in the scheduler, no exhaustiveness checking.
- **Enum**: Closed set, exhaustiveness checking, better Rust ergonomics. New effects require Rust changes.

Recommendation: start with an enum for the known effect types (ReadVar, Throw, LoopControl, Bracket). The scheduler's `bubble_effect` matches on the enum. If we need open-ended effects later, migrating to strings is mechanical — the scheduler logic doesn't change, only the matching.

## New flat table entries (Rust)

```rust
pub enum FlatAction {
    // ... existing variants ...

    /// Install a scoped effect handler.
    Handle {
        /// Maps effect type to the ActionId of the handler DAG.
        handlers: BTreeMap<EffectType, ActionId>,
        /// The body to execute within the handler's scope.
        body: ActionId,
    },

    /// Emit an effect. Pipeline value becomes the payload.
    Perform {
        effect: EffectType,
    },

    /// Resume a suspended continuation.
    /// Input: { cont_id: u32, value: Value }
    Resume,
}
```

## New frame kind (Rust scheduler)

```rust
pub enum FrameKind {
    // ... existing variants ...

    /// Scoped effect handler. Intercepts matching effects from the body.
    Handle {
        /// Maps effect type to handler ActionId.
        handlers: BTreeMap<EffectType, ActionId>,
        /// Active continuations: cont_id -> root ParentRef of suspended subgraph.
        continuations: HashMap<u32, ParentRef>,
        /// Counter for generating cont_ids.
        next_cont_id: u32,
    },
}
```

The Handle frame stores active continuations. When a Perform bubbles up to this frame, it:
1. Severs the parent link (creating a disconnected subgraph = the continuation)
2. Generates a cont_id and stores the continuation's root pointer
3. Advances the handler DAG with `{ payload, cont_id }`

When a Resume action executes:
1. Looks up cont_id in the nearest Handle frame's continuations map
2. Reconnects the subgraph's parent link
3. Delivers the value into the continuation's root frame

When the Handle frame exits (body completes):
1. Any un-resumed continuations are cleaned up (frames removed from slab)
2. The Handle frame delivers the body's result to its parent

## bubble_effect: the core traversal

```rust
impl WorkflowState {
    /// Walk parent pointers to find a Handle frame matching the effect.
    /// Intermediate nodes (Chain, Parallel, Branch, ForEach) are skipped.
    fn bubble_effect(
        &mut self,
        starting_frame: FrameId,
        effect: EffectType,
        payload: Value,
    ) -> Result<(), WorkflowError> {
        let mut current = self.frames[starting_frame].parent;
        let mut child_frame_id = starting_frame;

        while let Some(parent_ref) = current {
            let parent_id = parent_ref.frame_id();
            let parent = &self.frames[parent_id];

            if let FrameKind::Handle { handlers, .. } = &parent.kind {
                if handlers.contains_key(&effect) {
                    // Found matching handler.
                    // 1. Sever parent link of child_frame_id.
                    // 2. Store continuation in Handle frame.
                    // 3. Advance handler DAG with { payload, cont_id }.
                    return self.dispatch_to_handler(parent_id, child_frame_id, effect, payload);
                }
            }

            // Not handled here — keep walking up.
            child_frame_id = parent_id;
            current = parent.parent;
        }

        Err(WorkflowError::UnhandledEffect(effect))
    }
}
```

Key properties:
- Chain, Parallel, Branch, ForEach are completely unaware of effects. They sit dormant in the slab.
- The traversal is O(depth) where depth is the number of frames between Perform and Handle. In practice, this is small (< 20).
- If no Handle matches, it's an error. This is analogous to an unhandled exception.

## Continuation lifecycle

### Creation (on Perform)

1. `advance` evaluates a Perform node. Instead of returning a value, it calls `bubble_effect`.
2. `bubble_effect` walks up to the Handle frame.
3. The Handle frame severs the parent link of the frame immediately below it (the continuation root).
4. It assigns a cont_id and stores `(cont_id, continuation_root)` in its local map.
5. It advances the handler DAG with `{ payload, cont_id }`.

### Resumption (on Resume)

1. `advance` evaluates a Resume node. The pipeline value is `{ cont_id, value }`.
2. The scheduler looks up cont_id in the Handle frame's continuations map.
3. It reconnects the continuation root's parent link to the Handle frame.
4. It calls `deliver` on the continuation root with the value.
5. The dormant subgraph wakes up and execution proceeds normally.

### Discard (on Handle exit without Resume)

1. The Handle frame's body completes (or errors).
2. The Handle frame checks its continuations map for un-resumed entries.
3. For each un-resumed continuation:
   - Traverse downward from the root frame.
   - For Parallel/ForEach frames: recurse into children.
   - Cancel pending external tasks.
   - Remove all frames from the slab.
4. The Handle frame delivers the body's result (or error) to its parent.

### Discard (explicit, for Race-style patterns)

A handler DAG that wants to explicitly discard a continuation (without resuming) can pipe the cont_id into a `Discard` builtin (or simply not resume — the Handle frame cleans up on exit). For Race, the handler completes without resuming the losing branch. The Handle frame's exit cleanup handles it.

## Interaction with existing frame kinds

### Chain

No changes. If a child suspends (via bubble_effect walking past the Chain frame), the Chain frame sits dormant. When the continuation is resumed and the child eventually delivers a value, the Chain frame's existing `deliver` logic advances to the rest.

### Parallel

No changes. If child A suspends, A's slot in the Parallel frame's results vector remains empty. When the continuation is resumed and A delivers, the Parallel frame's existing logic checks if all slots are full and joins if so. This is structurally identical to waiting for an external Invoke to complete.

### Branch

No changes. Same pattern as Chain.

### ForEach

No changes. Same pattern as Parallel — each iteration is a slot, suspended iterations are empty slots.

### Loop (before Phase 4 migration)

During Phase 1, Loop still exists as a separate frame kind. It doesn't interact with Handle/Perform. After Phase 4, Loop is replaced by Handle(LoopControl).

## Test strategy

### Unit tests (Rust)

1. **Perform without Handle**: Verify `UnhandledEffect` error.
2. **Handle + Perform + Resume**: Simple effect that suspends and immediately resumes with a constant value. Verify the body produces the resumed value.
3. **Nested Handle**: Inner Handle intercepts one effect, outer Handle intercepts another. Verify correct routing.
4. **Handle + Perform in Chain**: Perform is the first half of a Chain. After resume, the rest of the Chain executes.
5. **Handle + Perform in Parallel**: One parallel branch performs an effect. The other completes normally. After resume, the Parallel joins both results.
6. **Discard on Handle exit**: Handler does not resume. Handle frame exits. Verify continuation frames are cleaned up from the slab.
7. **Multiple Performs**: Body performs multiple effects (sequentially). Each is handled and resumed. Verify all resume correctly.
8. **Perform across ForEach**: An action inside ForEach performs an effect. Verify the ForEach iteration suspends and resumes correctly.

### Integration tests

1. **End-to-end with TypeScript handler**: The handler DAG includes an Invoke that calls a TypeScript function. The function returns a value. The handler DAG pipes into Resume. The continuation receives the value.
2. **Nested handlers with TypeScript**: Multiple Handle blocks, TypeScript handlers at different levels.

## Deliverables

1. `EffectType` enum (or string, per decision above)
2. `FlatAction::Handle`, `FlatAction::Perform`, `FlatAction::Resume` in the flat table
3. `FrameKind::Handle` in the scheduler
4. `bubble_effect` traversal
5. Continuation management (create, resume, discard)
6. Slab cleanup on discard
7. Flattener support (tree AST Handle/Perform → flat table entries)
8. Tests per above

## What this phase does NOT include

- No semantic effects (ReadVar, Throw, LoopControl). Those come in Phases 2-4.
- No TypeScript surface API changes. Declare, tryCatch, loop rewriting come later.
- No RAII / Bracket. That's Phase 5.
- No durable suspension. That's Phase 6.

This phase builds infrastructure only. The test suite validates the mechanism using synthetic effects.
