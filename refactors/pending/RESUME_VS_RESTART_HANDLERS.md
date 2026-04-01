# Resume vs Restart Handlers

## The two kinds

Every Handle/Perform usage in Barnum falls into one of two categories based on what the handler does with the body's continuation:

| Kind | What the handler does | Body suspended? | Examples |
|------|----------------------|-----------------|----------|
| **ResumeHandle** | Always resumes with a value. Function call semantics. | No | `bind` (VarRef access), future `provide`/`consume` |
| **RestartHandle** | Tears down the body. Decides: re-enter (Continue) or exit (Break). | Yes | `tryCatch`, `race`, `withTimeout`, `loop`, `scope` |

Resume handlers resemble function calls: the body Performs, a value comes back, the body continues. The handler always produces a value for the Perform site — it never tears down the body.

Restart handlers control the body's fate. They tear down the body and decide: re-enter it with a new value (Continue) or exit the Handle entirely (Break). The handler has authority over the continuation.

## Restart handlers subsume resume

A restart handler that always Continues with the handler result and never Breaks is semantically similar to a resume handler. The current engine implements all handlers as restart — every Handle suspends the body, runs the handler DAG, inspects the HandlerOutput tag, and dispatches Resume/Discard/RestartBody.

For resume handlers, this suspension is unnecessary. The handler will always resume. There's no decision to make. The body doesn't need to be frozen because nothing will ever tear it down.

## Restart handlers can implement resume patterns

The loop pattern shows how restart handlers implement "variable binding" behavior:

```
Handle(jumpEffect, RestartBody,
  Branch({
    Continue: body,   // run the body
    Break: identity(), // exit
  })
)
```

One handler, RestartBody + state + branch. The handler restarts the body with a tagged value, and the branch at the top dispatches. This is a restart handler emulating iteration.

`bind` uses a different pattern — it resumes with the stored value:

```
ResumeHandle(varEffect,
  body_that_performs_varEffect
)
```

The handler reads from state and resumes. This is a resume handler: it always returns a value to the Perform site. The body is never torn down.

## The optimization: resume handlers don't suspend

For resume handlers, the execution flow is currently:

1. Body hits `Perform(effect_id)` with payload
2. Engine walks up the frame tree to find matching `Handle`
3. Engine **suspends** the body (sets `HandleStatus::Suspended(perform_parent)`)
4. Engine runs the handler DAG with `{ payload, state }`
5. Handler DAG completes with `HandlerOutput::Resume { value, state_update }`
6. Engine inspects the tag, sees Resume
7. Engine sets `HandleStatus::Free`
8. Engine delivers `value` back to `perform_parent`

Steps 3, 5–7 are unnecessary. The handler always Resumes. We know this statically — the handler DAG's output is always `Resume`. There's no branch to inspect.

The optimized flow:

1. Body hits `Perform(effect_id)` with payload
2. Engine walks up the frame tree to find matching `Handle`
3. Engine runs the handler DAG with `{ payload, state }` — **as a chain trampoline**
4. Handler DAG completes with a value
5. Engine delivers the value back to `perform_parent`

This is structurally identical to a Chain trampoline. Chain's `rest` is a statically known ActionId. A resume handler's "rest" is the handler DAG, found by walking up at runtime. But the mechanics are the same: remove a frame, advance the handler DAG, deliver the result when it completes.

### What we avoid

- **No suspension.** The ResumeHandle frame stays `Free` throughout. The body is never frozen.
- **No stash pressure.** The stash system exists because deliveries and effects can arrive while a Handle is suspended. Resume handlers never suspend, so their descendants never hit a suspended ancestor. No stashing needed for resume handler interactions.
- **No HandlerOutput inspection.** The handler DAG produces a raw value, not a tagged `Resume`/`Discard`/`RestartBody` wrapper. The engine delivers it directly. No deserialization of the wrapper, no match on the tag.

### What remains the same

- **Effect bubbling.** The Perform still walks up the frame tree to find the matching Handle. This walk is the "dynamic dispatch" — the cost of finding the handler at runtime instead of statically.
- **Handle state.** Resume handlers can still have state (bind uses it). State updates happen when the handler DAG completes.
- **Nesting.** Resume handlers compose with restart handlers and vice versa. A resume handler's body can contain other Handles.

## Engine representation

Two separate frame kinds — not one frame with a mode flag. Impossible states are unrepresentable.

```rust
/// Function-call semantics. Handler always resumes the body at the Perform site.
/// Never suspends. No HandleStatus needed.
pub struct ResumeHandleFrame {
    pub effect_id: EffectId,
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Option<Value>,
}

/// Control-flow semantics. Handler tears down the body and decides: re-enter (Continue) or exit (Break).
/// Body is suspended while the handler runs.
pub struct RestartHandleFrame {
    pub effect_id: EffectId,
    pub body: ActionId,
    pub handler: ActionId,
    pub state: Option<Value>,
    pub status: HandleStatus,
}
```

`ResumeHandleFrame`:
- No `status` field — it's always Free by construction
- The handler DAG produces a raw value, not a HandlerOutput envelope
- `dispatch_to_handler` skips suspension and runs the handler DAG as a chain-like trampoline
- `complete` for the handler side delivers the value directly to `perform_parent` without inspecting Resume/Discard/RestartBody

`RestartHandleFrame`:
- Current behavior. No changes.

## The trampoline analogy

Chain works like this:
1. The first child completes with a value
2. Chain frame removes itself
3. Engine advances `rest` with the value and the Chain's parent

A resume handler Perform works like this:
1. The Perform fires with a payload
2. Engine finds the matching Handle by walking up
3. Engine runs the handler DAG with `{ payload, state }` as a child of the Handle frame (handler side)
4. Handler DAG completes with a value
5. Engine delivers the value to the Perform's parent (back into the body)

The difference from Chain: step 2 is a runtime walk instead of a static ActionId. Everything else is the same mechanics. The Handle frame acts as a trampoline that routes the value through the handler DAG and back to the Perform site.

## TS surface

The mode is determined by the combinator, not by the user:

| Combinator | Handle kind | Why |
|-----------|------|-----|
| `bind` / `bindInput` | ResumeHandle | VarRef always resumes with stored value |
| `tryCatch` | RestartHandle | Handler Breaks (exits) on throw |
| `race` | RestartHandle | First Perform Breaks (exits) the body |
| `withTimeout` | RestartHandle | Built on race |
| `loop` | RestartHandle | Handler Continues (re-enters) on recur |
| `scope` / `jump` | RestartHandle | Handler Continues (re-enters) on jump |

The user never specifies the mode directly. Each combinator knows its own mode and emits the correct Handle variant (ResumeHandle or RestartHandle) in the AST.

If we ever expose raw `handle`/`perform` as a user-facing primitive, the mode would be explicit:

```ts
// Resume: handler is a function call, always resumes
handle.resume(effectId, handlerDag, body)

// Restart: handler controls the continuation
handle.restart(effectId, handlerDag, body)
```

## What this means for the handler DAG

### Resume handler DAG

The handler DAG produces a **raw value**. No `Tag("Resume")` wrapping. No `HandlerOutput` envelope.

```
// bind's handler DAG:
ExtractField("state") → ExtractIndex(n)
// produces: the nth bound variable's value
```

The engine takes this value and delivers it to the Perform's parent. Done.

### Restart handler DAG

The handler DAG produces a **LoopResult envelope** — a tagged value with `Continue` or `Break` as the kind. The engine inspects the tag and dispatches accordingly.

```
// tryCatch's handler DAG:
pipe(recovery, Tag("Discard"))
// produces: { kind: "Discard", value: recoveryResult }
```

This is the current behavior for all handlers.

## Unifying Discard and RestartBody

Discard and RestartBody share the same first step: tear down the body. They differ only in what happens after:

| HandlerOutput | Tear down body | Then what |
|---------------|---------------|-----------|
| RestartBody | Yes | Re-enter body with new input |
| Discard | Yes | Exit the Handle with a value |

This is Continue vs Break. RestartBody is "continue the loop." Discard is "break out of the loop." The handler output for restart handlers is `LoopResult<TContinueInput, TBreakOutput>`.

### The current three-variant model

```rust
enum HandlerOutput {
    Resume { value, state_update },      // resume handler: deliver to Perform site
    Discard { value },                   // restart handler: exit Handle
    RestartBody { value, state_update }, // restart handler: re-enter body
}
```

Three variants, but no current combinator mixes them. Each handler always does exactly one:

| Combinator | Always produces |
|-----------|----------------|
| `bind` | Resume |
| `tryCatch` | Discard |
| `race` | Discard |
| `loop` | RestartBody |
| `scope`/`jump` | RestartBody |

Resume is the resume handler case. Discard and RestartBody are the two restart handler cases. This falls naturally into:

### The unified two-mode model

- **ResumeHandle**: handler produces a raw value. Engine delivers to Perform site. (= Resume)
- **RestartHandle**: handler produces `LoopResult`. Engine dispatches:
  - `Continue(value)`: tear down body, re-enter with value. (= RestartBody)
  - `Break(value)`: tear down body, exit Handle with value. (= Discard)

Two separate frame kinds. Three HandlerOutput variants collapse into two frame types, where RestartHandle uses the LoopResult control flow enum — the same enum used by loop bodies, the same one used by scope/jump internals.

### How this simplifies loop compilation

**Current loop compilation** (from EFFECTS_PHASE_4_LOOP.md):

```
RestartHandle(jumpEffect,
  body: Branch({
    Continue: pipe(actualBody, ...),  // initial entry + recur
    Break: identity(),                 // done: exit normally
  }),
  handler: <always RestartBody>
)
```

The handler always RestartBodies. The Break case is a trick: RestartBody with a Break-tagged value, the Branch picks Break, identity() completes normally, and the Handle exits via normal body completion.

**Unified loop compilation:**

```
RestartHandle(jumpEffect,
  body: actualBody,  // no Branch wrapper needed
  handler: identity()  // pass through the LoopResult from the Perform payload
)
```

The Perform payload is already a LoopResult (recur tags as Continue, done tags as Break). The handler passes it through with `identity()`. The engine inspects the LoopResult:
- Continue: re-enter body with the continue value
- Break: exit Handle with the break value

No Branch-at-the-top trick. No identity() exit path. The LoopResult in the handler output drives the engine directly.

### How this simplifies tryCatch

**Current:** handler runs recovery, then wraps as `{ kind: "Discard", value: result }`.

**Unified:** handler runs recovery, then wraps as `LoopResult::Break(result)`.

```
RestartHandle(throwEffect,
  body: bodyWithThrowPerforms,
  handler: pipe(ExtractField("payload"), recovery, tag<LoopResultDef, "Break">("Break"))
)
```

Break = exit the Handle with the recovery result. The engine sees Break and exits. Same behavior, same LoopResult enum.

### How this simplifies race

**Current:** handler Discards with the first result.

**Unified:** handler tags the first result as Break.

```
RestartHandle(raceEffect,
  body: All(pipe(a, Perform(raceEffect)), pipe(b, Perform(raceEffect))),
  handler: pipe(ExtractField("payload"), tag<LoopResultDef, "Break">("Break"))
)
```

First Perform wins. Handler wraps as Break. Engine exits the Handle, tearing down the remaining concurrent branches.

### State updates

In the current model, RestartBody carries `state_update` but Discard doesn't. In the unified model, Continue (= RestartBody) may carry state updates. Break (= Discard) doesn't need them — the Handle is exiting, so state is discarded.

For resume handlers, state updates could be handled separately — the handler DAG produces `{ value, state_update }` and the engine destructures. Or state is read-only for resume handlers (true for bind, where state is set once by the All that computes bindings).

### Why this is the right unification

LoopResult already exists. It's already a TaggedUnion with a combinator namespace. Every restart handler's decision is "keep going" or "stop." That's Continue/Break. Using the same enum everywhere means:

1. One concept to learn: restart handlers produce LoopResult
2. One enum in the engine: no three-way match on Resume/Discard/RestartBody
3. Loop, tryCatch, race, scope all compile to the same handler output format
4. The handler DAG's output type is uniform: `LoopResult<TContinueInput, TBreakOutput>`

The resume/restart split cleanly separates "function call" (Resume) from "control flow decision" (Continue/Break).

## Implementation plan

1. Add `ResumeHandle` and `RestartHandle` as separate AST nodes in `barnum_ast` (Rust) and separate action kinds (TS)
2. Add `ResumeHandleFrame` and `RestartHandleFrame` as separate frame kinds in the engine
3. `dispatch_to_handler` dispatches on frame kind:
   - ResumeHandleFrame: run handler DAG as chain-like child, deliver raw result to perform_parent
   - RestartHandleFrame: current behavior (suspend, run handler DAG, inspect HandlerOutput)
4. `complete` handler-side dispatches on frame kind:
   - ResumeHandleFrame: deliver value directly, apply state update (if any)
   - RestartHandleFrame: current behavior (deserialize HandlerOutput, dispatch)
5. Update `bind` to emit `ResumeHandle`
6. Verify all existing restart handlers (tryCatch, race, loop) still work
7. Remove `Tag("Resume")` wrapping from bind's handler DAG

## Resume handlers as a general call mechanism

Since resume handlers are function calls — walk up, find the handler, get a value back — they share the same mechanics as other things function calls do in traditional languages.

### RAII / resource management

In C++ and Rust, RAII ties resource cleanup to scope exit. A destructor runs when the stack frame is popped, regardless of whether the function returned normally or unwound via exception/panic.

A ResumeHandle frame is a scope with a lifetime. When the body completes (normally or via a restart handler's Break above it), the Handle frame is torn down. If resume handlers had a **cleanup action** that runs on frame teardown, you'd get RAII:

```ts
withResource(
  (resource) => pipe(
    resource.get(),   // resume Perform: reads the resource value
    doWork,
  ),
  { create: acquireDb, dispose: releaseDb }
)
```

The Handle frame would:
1. Run `create` to acquire the resource, store in state
2. Run the body — `resource.get()` is a resume Perform that reads from state
3. On body completion OR on body teardown (Break from an outer restart handler), run `dispose`

Step 3 is the RAII guarantee: cleanup runs regardless of exit path. The current `withResource` combinator (in builtins.ts) desugars to a chain of All + Merge + extractIndex, which doesn't handle the teardown-on-Break case. A resume handler with a cleanup action would handle it naturally because the Handle frame's teardown hook fires whenever the frame is removed.

This would require a small engine addition: an optional `on_teardown: ActionId` on HandleFrame that the engine advances (with the state as input) during `teardown_body` or when the Handle frame itself is removed. The cleanup action runs as a "finally" block.

### Provide/Consume (dynamic scope)

Resume handlers ARE Provide/Consume. `bind` provides values; VarRef Performs consume them. A general `provide(name, value, body)` is a resume handler where the handler DAG returns the provided value on every Perform.

The connection: dynamic scope in traditional languages is implemented as a stack walk — `consume("x")` walks the call stack looking for the nearest binding of `x`. Resume handler Perform does the same thing — it walks the frame tree looking for the matching Handle. The mechanics are identical.

### Capabilities / tokens

A resume handler that returns a capability token is an **effect-scoped capability**. The token is only valid within the Handle's body — Performing outside the scope hits `UnhandledEffect`. This is how `tryCatch`'s `throwError` token works (though that's a restart handler). A resume handler version would be: "here's a logger/db/auth token, use it freely within this scope, it's cleaned up when the scope exits."

## Can every primitive be a Handle?

If resume and restart handlers are the two fundamental operations, how many of the other AST primitives can be reduced to Handle/Perform? Here's the strongest case for each one.

### Invoke → resume Perform to a root handler

Invoke sends a value to an external TypeScript handler and gets a value back. That's exactly resume handler semantics: Perform, get a value, continue.

Model: the runtime installs a root-level ResumeHandle that wraps the entire workflow. Every Invoke becomes a Perform targeting this root handler. The Perform payload includes the HandlerId (which handler to call) and the value. The root handler dispatches to the external TypeScript subprocess and resumes with the result.

```
// The entire workflow becomes:
ResumeHandle(invokeEffect,
  body: <workflow where every Invoke is replaced with Perform(invokeEffect)>,
  handler: dispatch_to_runtime(payload.handler_id, payload.value)
)
```

The Perform payload is `{ handler_id, value }`. The root handler is the syscall boundary — the one place where the engine yields to the external runtime.

**This works cleanly.** Invoke and resume Perform have identical semantics: send a value out, get a value back, continue. The only difference is that Invoke statically names its handler (HandlerId in the flat table) while Perform carries the handler identity in its payload. The flattener would pack the HandlerId into the Perform payload at compile time.

**What we gain:** a unified model. "Getting a value from somewhere" is always Perform. Whether "somewhere" is a resume handler's state (bind), a Rust builtin (extractField), or an external TypeScript process (current Invoke) — it's all the same mechanism. The root handler is the interpreter for external effects.

**What we lose:** nothing significant. The root handler is always installed. The engine doesn't need special Invoke logic — it's just another ResumeHandle. Dispatch overhead is one frame-tree walk per Invoke, but the root Handle is always the outermost frame, so the walk is O(depth) where depth is the number of nested Handles.

**Verdict: compelling.** Invoke is the most natural candidate for Handle reduction.

### Loop → RestartHandle (already designed)

Already covered in EFFECTS_PHASE_4_LOOP.md. Loop is a RestartHandle with Continue to re-enter and Break to exit. The handler DAG tags as LoopResult and the body Branch dispatches. One effect, one handler. The LoopAction AST node can be removed entirely.

**Verdict: done.** This is a clean, designed reduction.

### Step → restart Perform to a named scope

Step is an unconditional jump to a named location. If each named step is wrapped in a RestartHandle at the top level, then `step("Cleanup")` is a Perform that bubbles up to the Cleanup Handle. The handler Breaks with the step's action result, exiting the Handle.

```
// registerSteps({ Validate: ..., Process: ... }) compiles to:
RestartHandle(validateEffect,
  body: RestartHandle(processEffect,
    body: <the workflow>,
    handler: <Process step action, then Break>
  ),
  handler: <Validate step action, then Break>
)
```

`step("Validate")` becomes `Perform(validateEffect)`. The Perform bubbles up through the Process Handle (non-matching effect_id, skipped) and reaches the Validate Handle. The handler runs Validate's action and Breaks.

**Where it works:** top-level step references where the step is an ancestor in the frame tree. `scope`/`jump` already proves this pattern — jump is a Perform targeting a scope handler.

**Where it breaks:** mutual recursion. If step A jumps to step B and step B jumps to step A, both need to be ancestors of each other — impossible in a tree. Currently, Step is a flat table goto that jumps anywhere regardless of frame tree structure.

The workaround: mutual recursion becomes a RestartBody loop with a state machine. Instead of "A jumps to B, B jumps to A," you have a loop at the top that dispatches on a `{ kind: "RunA" | "RunB", value }` tagged union. Both A and B Perform to the loop's handler with the appropriate tag, and the handler RestartBodies.

```
RestartHandle(stepEffect,
  Branch({
    RunA: pipe(<A's body>, branch({ goToB: pipe(tag("RunB"), Perform(stepEffect)), ... })),
    RunB: pipe(<B's body>, branch({ goToA: pipe(tag("RunA"), Perform(stepEffect)), ... })),
  })
)
```

This is more structured than goto — mutual recursion is expressed as a state machine rather than arbitrary jumps. It's arguably better: the set of reachable states is visible in the Branch cases.

**What we lose:** O(1) jumps. Step is a direct index into the flat table. Perform walks the frame tree. For deeply nested workflows, this is slower. But step jumps are typically to top-level steps, which are the outermost Handles, so the walk depth is bounded by the number of registered steps.

**Verdict: viable, with a structural change for mutual recursion.** The state machine encoding is more restrictive but more analyzable than arbitrary goto.

### Branch → RestartHandle with case dispatch

Branch dispatches on `{ kind, value }`. Could it be a Handle where the body Performs with the value, and the handler inspects the kind and runs the matching case?

```
// branch({ Ok: handle, Err: recover }) compiles to:
RestartHandle(branchEffect,
  body: Perform(branchEffect),      // send the value to the handler
  handler: <inspect kind, run matching case, Break>
)
```

The handler receives the tagged value, reads the `kind` field, and runs one of N case actions. It Discards with the case action's output.

**The problem:** the handler DAG needs to do the dispatching. How does the handler DAG dispatch on `kind` without using Branch? If the handler DAG contains a Branch node, we've just moved Branch from the main AST to the handler DAG — not eliminated it.

**The steel-man:** make case dispatch a native capability of Handle. Instead of a single handler DAG, the Handle carries a **case map**: `Record<string, ActionId>`. The engine reads `value.kind`, looks up the matching ActionId in the case map, and runs it. This is Branch's logic built into the Handle frame.

```rust
pub enum HandlerDag {
    /// Single DAG: for effects (tryCatch, bind, etc.)
    Single(ActionId),
    /// Case dispatch: for branching
    Cases(BTreeMap<String, ActionId>),
}
```

When a Perform fires and the handler is `Cases(map)`, the engine:
1. Reads `payload.kind`
2. Looks up `map[kind]`
3. Runs the matching ActionId
4. Breaks with the result

This integrates Branch into Handle cleanly. Branch is no longer a separate AST node — it's a Handle with a case-dispatch handler.

**What we gain:** one fewer AST node. Branching and effect handling share the same frame infrastructure. Static analysis sees all dispatch points as Handle frames.

**What we lose:** conceptual clarity. Branch is simple: read a field, jump. Handle is complex: effect scopes, suspension, stashing. Merging them forces simple dispatch through the heavy machinery. The engine's Handle code path gets more complex (must handle both single-DAG and case-dispatch), and the case-dispatch path doesn't benefit from any of Handle's effect features.

**Counter-argument to the loss:** resume handlers don't have the heavy machinery. A ResumeHandle with case dispatch doesn't suspend, doesn't stash, doesn't inspect HandlerOutput. It's: find the Handle, read the kind, jump to the case action. That's as lightweight as current Branch — just with a frame-tree walk to find the Handle.

But why walk the frame tree when the Branch is right here? Branch doesn't need to be "found" — it's the next action in the pipeline. The frame-tree walk adds overhead for no benefit when the dispatch is local.

**Verdict: technically possible but forced.** Branch-as-Handle works mechanically, but the frame-tree walk overhead and conceptual conflation don't justify it. Branch is a pure local operation; Handle is a scoped mechanism. They serve different purposes even if you can encode one in the other.

### Chain → Handle body completion

Chain is "run A, then run B with A's result." The simplest sequencing primitive.

**The argument:** when a Handle's body completes normally, the Handle could advance to a "continuation" action instead of just delivering the value to its parent. If Handle has an `on_complete: Option<ActionId>`, then Chain is:

```
ResumeHandle(_, body: A, on_complete: B)
```

A runs. A completes. Handle runs B with A's result. B completes. Handle delivers B's result to its parent.

For a 3-step chain `pipe(A, B, C)`:

```
ResumeHandle(_, body: A, on_complete:
  ResumeHandle(_, body: B, on_complete: C))
```

**The problem:** this is O(N) frames for N-step chains. Chain's tail-call optimization gives O(1) frames. When Chain's first child completes, the Chain frame removes itself and trampolines to `rest` — no frame accumulates. Handle doesn't have this optimization (the Handle frame persists across the body's execution).

**The fix:** give Handle the same tail-call optimization for `on_complete`. When the body completes and there's an `on_complete`, the Handle frame removes itself and trampolines to `on_complete`. This is exactly Chain's trampoline.

But now Handle's `on_complete` path IS Chain. We haven't eliminated Chain — we've absorbed its implementation into Handle. The engine code for "body completed, advance to continuation" is identical whether it's in a Chain frame or a Handle frame with `on_complete`.

**A further argument:** maybe Chain doesn't need to be a frame kind at all. What if sequencing is the engine's fundamental dispatch mechanism, not a frame kind?

Currently, Chain creates a frame. The frame stores `rest`. When the first child completes, the frame trampolines. But the trampoline is a `(value, action_id, parent)` tuple that feeds back into `advance()`. What if `advance()` itself handles sequencing — when it finishes expanding an action and the action has a `rest`, it trampolines directly, without ever creating a Chain frame?

In the flat IR, this would mean: every multi-entry action (Chain, Handle, All, Branch) has its children as inlined entries. When the engine advances an action that has a `rest` field, it records the rest as a trampoline target. No frame created.

This is how Chain already works — the Chain frame is just a trampoline vehicle. The frame exists to hold `rest` and `parent` until the first child completes. If the engine can thread this trampoline through without a frame, Chain disappears from the frame kinds.

**But this is an engine optimization, not a semantic reduction.** Chain's semantics (sequence two actions) still need to be expressed somehow. Whether as a frame kind, a trampoline in `advance()`, or an `on_complete` on Handle — the sequencing logic exists. We're moving it, not removing it.

**Verdict: Chain is irreducible as a concept.** You can merge its implementation into Handle or into the engine's advance loop, but sequencing doesn't go away. It's one of the three fundamental operations (sequence, branch, concurrent) that any control flow system needs.

### All → concurrent spawn/join effects

All fans out to N concurrent children and collects their results. Handle has one body. Can we bridge this gap?

**The argument:** model concurrency as effects.

```ts
// User writes:
all(computeA, computeB, computeC)

// Compiles to:
ResumeHandle(spawnEffect, state: { results: [], expected: 3 },
  body: pipe(
    // Spawn three concurrent tasks
    fork(computeA, computeB, computeC),
    // Fork is a Perform that tells the runtime to run them concurrently
    Perform(spawnEffect),
  ),
  handler: <collect results into state, Resume with results when all done>
)
```

Where `fork(a, b, c)` packages the three actions into a single "please run these concurrently" effect payload. The root concurrent handler receives this payload, spawns three concurrent evaluation contexts, collects results, and Resumes with the tuple.

**This is how Koka models concurrency.** In Koka, `async` and `await` are effects. The handler decides whether to run things concurrently (with a thread pool handler) or sequentially (with a sequential handler). The program doesn't know — it just emits spawn/join effects.

**The key insight:** if the root invoke handler is already intercepting all external dispatches, it can also manage concurrency. Multiple concurrent Invokes from different branches of the body are already handled by the runtime's event loop — the engine produces multiple Dispatches and the runtime processes them concurrently. All's frame logic (collecting results into slots) is just bookkeeping. Could the runtime do this bookkeeping?

**Where it works:** the runtime already manages concurrent Invoke dispatches. If the "fork" effect sends N actions to the runtime, the runtime can run them concurrently and return the collected results as a single value. From the engine's perspective, it's one resume Perform (the fork) that produces one value (the collected tuple). No frame-tree fan-out needed.

**Where it breaks:** effects inside the concurrent branches. If `computeA` contains a `Perform(throwEffect)` that should be caught by an enclosing `tryCatch`, the Perform needs to bubble up through the frame tree. But if `computeA` is running in the runtime (outside the engine), there's no frame tree to bubble through. The concurrent branches are detached from the engine's frame tree.

This is the key tension. All's current implementation keeps concurrent branches inside the frame tree, which means effects bubble correctly. Moving concurrency to the runtime breaks effect composition.

**The fix:** don't move concurrency to the runtime. Keep it in the engine, but express it as a Handle feature. Extend Handle to support **multiple concurrent bodies**:

```rust
pub enum HandleBody {
    /// Single body (normal effects: tryCatch, bind, etc.)
    Single(ActionId),
    /// Concurrent bodies (replaces All)
    Concurrent(Vec<ActionId>),
}
```

When `HandleBody::Concurrent`, the Handle frame fans out to all bodies concurrently, collects results into slots (exactly what All does), and when all complete, delivers the tuple to the handler or directly to the parent.

This merges All into Handle. The Handle frame now knows how to manage concurrent children. All is no longer a separate frame kind — it's a Handle with a concurrent body.

**What we gain:** All's structured concurrency guarantees come from Handle's teardown semantics. When a Handle Breaks (exits), all body children are torn down — including concurrent branches. This is already how `race` works (Handle wrapping an All). Making All a Handle feature means every concurrent fan-out gets automatic teardown on Break, which is structured concurrency by construction.

**What we lose:** simplicity. Handle is already the most complex frame kind. Adding concurrent body support makes it more complex. All is currently simple: N slots, fill slots, deliver when full. Merging this into Handle means Handle's frame logic branches on Single vs Concurrent body mode.

**Verdict: viable and arguably elegant.** Concurrent bodies as a Handle feature unifies fan-out with effect handling. The complexity cost is real but bounded — it's one more enum variant in HandleBody.

### ForEach → concurrent spawn over runtime array

ForEach is All applied to a dynamic-length array. The N isn't known at compile time.

If All becomes Handle-with-concurrent-bodies, ForEach needs dynamic concurrent bodies — the Handle creates N bodies at runtime based on the input array's length.

This requires the engine to dynamically instantiate action trees — running the same action body once for each array element. Currently ForEach does exactly this. In a Handle-based model, the Handle frame would:

1. Read the input array
2. Create N concurrent body evaluations (one per element)
3. Collect N results
4. Deliver the result array

This is the same logic as ForEach's current frame, just housed in a Handle frame. The reduction is mechanical — rename ForEach to Handle-with-dynamic-concurrent-body.

**Verdict: follows from All.** If All becomes a Handle feature, ForEach extends it with dynamic body count.

### Summary: the minimal primitive set

With resume and restart handlers carrying concurrent body support, the primitives reduce to:

| Current primitive | Reduced to | Clean? |
|------------------|-----------|--------|
| **Invoke** | Resume Perform to root handler | Yes — natural fit |
| **Loop** | RestartHandle with Continue/Break | Yes — already designed |
| **Step** | Restart Perform to named scope handler | Mostly — mutual recursion needs state machine encoding |
| **Branch** | RestartHandle with case-dispatch handler | Technically yes — but adds overhead for a local operation |
| **Chain** | Sequencing within Handle body / on_complete | No — Chain's semantics are irreducible; can be absorbed but not eliminated |
| **All** | Handle with concurrent body | Yes — unifies fan-out with effect scoping |
| **ForEach** | Handle with dynamic concurrent body | Follows from All |

The **genuinely irreducible** concepts:
1. **Sequencing** — actions must execute in order. Whether this is Chain frames, Handle on_complete, or engine-level trampolining, the sequencing logic exists somewhere.
2. **Effect handling** — Handle/Perform (resume and restart).
3. **Concurrency** — running N things at once. Can be absorbed into Handle as concurrent bodies, but the concurrent execution mechanism must exist.

Everything else is derivable. The maximally reduced AST would have:

```
FlatAction =
  | ResumeHandle { effect_id, body: HandleBody, handler: HandlerDag }
  | RestartHandle { effect_id, body: HandleBody, handler: HandlerDag }
  | Perform { effect_id }
  | Chain { rest: ActionId }   // irreducible sequencing
```

With Handle absorbing All (concurrent bodies), ForEach (dynamic concurrent bodies), Branch (case-dispatch handler), and Loop (Continue/Break). And Invoke absorbed into Perform targeting a root handler.

Four node types. Everything else is configuration on ResumeHandle/RestartHandle.

## Open questions

1. **State updates for resume handlers.** Resume handlers can have state (bind uses it). But the current state update mechanism is part of the HandlerOutput envelope (`Resume { state_update }`). For resume handlers, we need a different mechanism — either the handler DAG produces a `{ value, state_update }` tuple that the engine destructures, or state is read-only for resume handlers (which is fine for bind, where state is set once and never updated).

2. **Resume handler error semantics.** What happens if a resume handler's DAG fails (e.g., a TypeScript handler inside the DAG throws)? For restart handlers, the body is already suspended, so the engine can propagate the error upward. For resume handlers, the body is NOT suspended — it's still "running" (from the frame tree's perspective). The handler failure needs to propagate through the Handle frame and up to the Handle's parent, same as if the body itself had failed. This should work naturally — the handler DAG is a child of the Handle frame, so errors propagate upward through the Handle.
