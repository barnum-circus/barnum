# Handle as Universal Primitive

## Premise

With resume and restart handlers as two distinct frame kinds (see RESUME_VS_RESTART_HANDLERS.md), Handle/Perform covers function-call semantics (resume) and control-flow semantics (restart). How many of the other AST primitives can be reduced to Handle/Perform?

## Invoke as resume Perform to a root handler

Invoke sends a value to an external TypeScript handler and gets a value back. That's resume handler semantics: Perform, get a value, continue.

The runtime installs a root-level ResumeHandle wrapping the entire workflow. Every Invoke becomes a Perform targeting this root handler. The Perform payload includes the HandlerId and the value. The root handler dispatches to the external TypeScript subprocess and resumes with the result.

```
ResumeHandle(invokeEffect,
  body: <workflow where every Invoke is replaced with Perform(invokeEffect)>,
  handler: dispatch_to_runtime(payload.handler_id, payload.value)
)
```

The Perform payload is `{ handler_id, value }`. The root handler is the syscall boundary. Invoke and resume Perform have identical semantics: send a value out, get a value back, continue. The only difference is that Invoke statically names its handler (HandlerId in the flat table) while Perform carries the handler identity in its payload. The flattener would pack the HandlerId into the Perform payload at compile time.

What we gain: a unified model. "Getting a value from somewhere" is always Perform. Whether "somewhere" is a resume handler's state (bind), a Rust builtin (extractField), or an external TypeScript process (current Invoke), it's the same mechanism. The root handler is the interpreter for external effects.

What we lose: nothing significant. Dispatch overhead is one frame-tree walk per Invoke, but the root Handle is the outermost frame, so the walk is O(depth) where depth is the number of nested Handles.

**Verdict: compelling.** Invoke is the most natural candidate for Handle reduction.

## Loop as RestartHandle (already designed)

Covered in EFFECTS_PHASE_4_LOOP.md. Loop is a RestartHandle with Continue to re-enter and Break to exit. The handler DAG tags as LoopResult and the body Branch dispatches. The LoopAction AST node can be removed entirely.

**Verdict: done.**

## Step as restart Perform to a named scope

Step is an unconditional jump to a named location. If each named step is wrapped in a RestartHandle at the top level, then `step("Cleanup")` is a Perform that bubbles up to the Cleanup Handle. The handler Breaks with the step's action result.

```
RestartHandle(validateEffect,
  body: RestartHandle(processEffect,
    body: <the workflow>,
    handler: <Process step action, then Break>
  ),
  handler: <Validate step action, then Break>
)
```

`step("Validate")` becomes `Perform(validateEffect)`. The Perform bubbles through non-matching Handles and reaches the Validate Handle. The handler runs Validate's action and Breaks.

Where it works: top-level step references where the step is an ancestor in the frame tree. `scope`/`jump` already proves this pattern.

Where it breaks: mutual recursion. If step A jumps to step B and step B jumps to step A, both need to be ancestors of each other, which is impossible in a tree. Currently, Step is a flat table goto.

The workaround: mutual recursion becomes a RestartBody loop with a state machine. Instead of arbitrary jumps, a loop at the top dispatches on a `{ kind: "RunA" | "RunB", value }` tagged union. Both A and B Perform to the loop's handler, and the handler RestartBodies with the appropriate tag.

```
RestartHandle(stepEffect,
  Branch({
    RunA: pipe(<A's body>, branch({ goToB: pipe(tag("RunB"), Perform(stepEffect)), ... })),
    RunB: pipe(<B's body>, branch({ goToA: pipe(tag("RunA"), Perform(stepEffect)), ... })),
  })
)
```

What we lose: O(1) jumps. Step is a direct index into the flat table. Perform walks the frame tree. But step jumps are typically to top-level steps (outermost Handles), so walk depth is bounded by the number of registered steps.

**Verdict: viable, with a structural change for mutual recursion.** The state machine encoding is more restrictive but more analyzable than arbitrary goto.

## Branch as Handle with case dispatch

Branch dispatches on `{ kind, value }`. Could it be a Handle where the body Performs with the value, and the handler inspects the kind and runs the matching case?

```
RestartHandle(branchEffect,
  body: Perform(branchEffect),
  handler: <inspect kind, run matching case, Break>
)
```

The problem: the handler DAG needs to dispatch on `kind`. If the handler DAG contains a Branch node, we've moved Branch from the main AST to the handler DAG, not eliminated it.

The steel-man: make case dispatch a native capability of Handle. Instead of a single handler DAG, the Handle carries a case map: `Record<string, ActionId>`. The engine reads `value.kind`, looks up the matching ActionId, and runs it.

```rust
pub enum HandlerDag {
    Single(ActionId),
    Cases(BTreeMap<String, ActionId>),
}
```

What we gain: one fewer AST node. Branching and effect handling share the same frame infrastructure.

What we lose: conceptual clarity. Branch is a pure local operation (read a field, jump). Routing it through Handle adds a frame-tree walk for no benefit when the dispatch is local. Resume handlers avoid the heavy machinery (no suspension, no stashing), so it would be lightweight. But the question remains: why walk the frame tree when the Branch is right here?

**Verdict: technically possible but forced.** Branch is local; Handle is scoped. Encoding one in the other doesn't justify the overhead.

## Chain as Handle body completion

Chain is "run A, then run B with A's result." If Handle had an `on_complete: Option<ActionId>`, Chain would be:

```
ResumeHandle(_, body: A, on_complete: B)
```

The problem: this is O(N) frames for N-step chains. Chain's tail-call optimization gives O(1) frames (the Chain frame removes itself and trampolines to `rest`). Handle frames persist across body execution.

The fix: give Handle the same tail-call optimization for `on_complete`. But then Handle's `on_complete` path IS Chain. We haven't eliminated Chain; we've absorbed its implementation.

A further argument: maybe Chain doesn't need to be a frame kind. Sequencing could be the engine's fundamental dispatch mechanism in `advance()`. When an action has a `rest` field, the engine trampolines directly without creating a frame. This is already how Chain works; the frame is just a trampoline vehicle.

But this is an engine optimization, not a semantic reduction. Chain's semantics (sequence two actions) still need expression somewhere.

**Verdict: Chain is irreducible as a concept.** Sequencing can be absorbed but not eliminated. It's one of three fundamental operations (sequence, branch, concurrent) that any control flow system needs.

## All as Handle with concurrent bodies

All fans out to N concurrent children and collects their results. Handle has one body.

One approach: model concurrency as effects. `fork(a, b, c)` becomes a Perform that tells the runtime to run actions concurrently. The Koka model.

Where it breaks: effects inside concurrent branches. If `computeA` contains a `Perform(throwEffect)` that should be caught by an enclosing `tryCatch`, the Perform needs to bubble through the frame tree. If `computeA` runs in the runtime (outside the engine), there's no frame tree to bubble through.

The fix: keep concurrency in the engine but express it as a Handle feature. Extend Handle to support multiple concurrent bodies:

```rust
pub enum HandleBody {
    Single(ActionId),
    Concurrent(Vec<ActionId>),
}
```

When `HandleBody::Concurrent`, the Handle frame fans out to all bodies, collects results into slots (exactly what All does), and delivers the tuple when all complete.

What we gain: All's structured concurrency guarantees come from Handle's teardown semantics. When a Handle Breaks, all body children are torn down. This is already how `race` works (Handle wrapping an All). Making All a Handle feature means every concurrent fan-out gets automatic teardown on Break: structured concurrency by construction.

What we lose: Handle is already the most complex frame kind. Adding concurrent body support adds one more branch to its logic.

**Verdict: viable and arguably elegant.**

## ForEach follows from All

ForEach is All applied to a dynamic-length array. If All becomes Handle-with-concurrent-bodies, ForEach extends it with dynamic body count. The Handle creates N body evaluations at runtime based on the input array's length. Mechanical reduction.

**Verdict: follows from All.**

## Summary: the minimal primitive set

| Current primitive | Reduced to | Clean? |
|------------------|-----------|--------|
| **Invoke** | Resume Perform to root handler | Yes |
| **Loop** | RestartHandle with Continue/Break | Yes (already designed) |
| **Step** | Restart Perform to named scope handler | Mostly (mutual recursion needs state machine) |
| **Branch** | RestartHandle with case-dispatch handler | Forced (adds overhead for a local operation) |
| **Chain** | Sequencing within Handle body / on_complete | No (irreducible concept) |
| **All** | Handle with concurrent body | Yes |
| **ForEach** | Handle with dynamic concurrent body | Follows from All |

The genuinely irreducible concepts:

1. **Sequencing** -- Chain frames, Handle on_complete, or engine-level trampolining. The logic exists somewhere.
2. **Effect handling** -- Handle/Perform (resume and restart).
3. **Concurrency** -- running N things at once. Can be absorbed into Handle as concurrent bodies, but the mechanism must exist.

The maximally reduced AST:

```
FlatAction =
  | ResumeHandle { resume_handler_id: ResumeHandlerId, body: HandleBody, handler: HandlerDag }
  | RestartHandle { restart_handler_id: RestartHandlerId, body: HandleBody, handler: HandlerDag }
  | BreakHandle { break_handler_id: BreakHandlerId, body: HandleBody, handler: HandlerDag }
  | ResumePerform { resume_handler_id: ResumeHandlerId }
  | RestartPerform { restart_handler_id: RestartHandlerId }
  | BreakPerform { break_handler_id: BreakHandlerId }
  | Chain { rest: ActionId }
```

Seven node types. Handle absorbs All (concurrent bodies), ForEach (dynamic concurrent bodies), Branch (case-dispatch handler), and Loop (RestartHandle + body-level Branch for break). Invoke becomes ResumePerform targeting a root ResumeHandle.

## Resume handlers as a general call mechanism

Resume handlers are function calls: walk up, find the handler, get a value back. This is the same mechanics as several other patterns.

### RAII / resource management

A ResumeHandle frame is a scope with a lifetime. When the body completes (normally or via a restart handler's Break above it), the Handle frame is torn down. If resume handlers had a cleanup action that runs on frame teardown, you'd get RAII:

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
2. Run the body -- `resource.get()` is a resume Perform that reads from state
3. On body completion OR on body teardown (Break from an outer restart handler), run `dispose`

Step 3 is the RAII guarantee: cleanup runs regardless of exit path. The current `withResource` combinator (in builtins.ts) desugars to a chain of All + Merge + extractIndex, which doesn't handle the teardown-on-Break case. A resume handler with a cleanup action would handle it naturally because the Handle frame's teardown hook fires whenever the frame is removed.

This would require an optional `on_teardown: ActionId` on HandleFrame that the engine advances (with the state as input) during `teardown_body` or when the Handle frame itself is removed.

### Provide/Consume (dynamic scope)

Resume handlers are Provide/Consume. `bind` provides values; VarRef Performs consume them. A general `provide(name, value, body)` is a resume handler where the handler DAG returns the provided value on every Perform.

Dynamic scope in traditional languages is a stack walk: `consume("x")` walks the call stack looking for the nearest binding of `x`. Resume handler Perform does the same thing: it walks the frame tree looking for the matching Handle.

### Capabilities / tokens

A resume handler that returns a capability token is an effect-scoped capability. The token is only valid within the Handle's body; Performing outside the scope hits `UnhandledEffect`. A resume handler version: "here's a logger/db/auth token, use it freely within this scope, it's cleaned up when the scope exits."
