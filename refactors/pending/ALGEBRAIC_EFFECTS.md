# Algebraic Effects as the Unifying Primitive

## The diagnosis

Barnum's current AST is grounded in Cartesian Monoidal Categories: `Chain` (composition), `Parallel` (tensor product), `Branch` (coproduct elimination), `ForEach` (functorial map), `Invoke` (external computation). This is a point-free, dataflow topology. Data flows through the graph without names.

The prop drilling problem is the fundamental weakness of point-free topologies. When step C needs the output of step A and step A's output isn't on the direct pipeline path to step C, you must manually route it (augment, tap, pick). This isn't a missing feature. It's a structural property of point-free composition.

The proposed `declare` + `VarRef` introduces the Lambda Calculus: named bindings, lexical scope, an environment. This is point-ful evaluation bolted onto a Cartesian structure. Rather than bolt on ad-hoc nodes for each new scope feature, we can unify all scope-based features through a single mechanism.

## The minimal AST

The goal: fewest possible primitives, maximum expressivity. Every scope-based feature (`declare`, `loop`, `tryCatch`, `withTimeout`, `race`, RAII) is TypeScript surface sugar that compiles down to these primitives.

| Node | Role | Categorical analogue |
|---|---|---|
| `Invoke` | External computation (handler call) | Morphism |
| `Chain` | Sequential composition | Kleisli composition |
| `Parallel` | Concurrent fork-join | Tensor product |
| `Branch` | Dispatch on tagged union | Coproduct elimination |
| `ForEach` | Map over array | Functorial lift |
| `Handle` | Install scoped effect handler | Effect handler |
| `Perform` | Emit effect, suspend execution | Effect operation |

7 nodes. `Loop` and `Step` are gone from the tree AST. (Step may remain in the flat table as a jump optimization.) Everything that creates a scope — variables, error handling, timeouts, resource management, looping — compiles to `Handle`/`Perform`.

Note: `pipe`, `augment`, `tap`, `pick`, `merge`, `option.map`, `declare`, `loop`, `tryCatch`, `withTimeout`, `race` are all TypeScript surface functions that produce trees of these 7 nodes. This is already the pattern — `pipe(a, b, c)` compiles to nested `Chain` nodes. Handle/Perform extends the same approach to scope-based features.

## How Handle/Perform works

- **`Handle(effect_type, handler_logic, body)`**: Wraps a body. When anything inside the body performs a matching effect, control transfers to the handler. The handler receives the effect's payload and a continuation (the suspended rest of the computation).
- **`Perform(effect_type, payload)`**: Suspends execution and yields to the nearest enclosing Handle for that effect type.

The handler can:
- **Resume the continuation** with a value (variable lookup: look up value, resume)
- **Discard the continuation** (error handling: don't resume, run recovery instead)
- **Re-enter the body** (loop: on Continue, restart the handler's body with a new input)

### Declare as Handle/Perform

```
// User writes:
declare({ x: computeX }, ({ x }) => body_using_x)

// Compiles to:
Chain(
  computeX,                           // evaluate binding eagerly
  Handle(
    effect: ReadVar,
    handler: on ReadVar(id) {
      // Each Handle frame holds one (id, value) pair.
      // If id matches, resume with the stored value.
      // If not, re-perform (propagate to outer Handle).
    },
    body: /* body with VarRefs replaced by Perform(ReadVar(id)) */
  )
)
```

The binding is evaluated before the Handle is entered. The Handle frame stores one `(id, value)` pair. VarRefs in the body become `Perform(ReadVar(id))`. When the scheduler encounters a Perform, it walks up the frame tree to find the Handle that owns that id. The Handle resumes the continuation with the value.

For the object form (concurrent bindings), the compilation wraps the bindings in Parallel first:

```
// declare({ a: exprA, b: exprB }, body)
// Compiles to:
Chain(
  Parallel(exprA, exprB),            // concurrent evaluation
  Handle(ReadVar("__0"), {           // bind first result
    Handle(ReadVar("__1"), {         // bind second result
      body
    })
  })
)
```

For the array form (sequential, dependent bindings), each binding is a nested Chain + Handle:

```
// declare([{ a: exprA }, ({ a }) => ({ b: exprB_using_a })], body)
// Compiles to:
Chain(
  exprA,
  Handle(ReadVar("__0"), {
    Chain(
      exprB_using_a,                  // may contain Perform(ReadVar("__0"))
      Handle(ReadVar("__1"), {
        body
      })
    )
  })
)
```

### Loop as Handle/Perform

```
// loop(body)
// Compiles to:
Handle(
  effect: LoopControl,
  handler: {
    on Continue(value) => re-enter body with value
    on Break(value) => return value (discard continuation, exit handler)
  },
  body  // body contains Perform(Continue(v)) or Perform(Break(v))
)
```

The current `LoopAction` frame already does exactly this. The handler re-enters its body on Continue (multi-shot: the continuation is discarded, a fresh execution starts) and exits on Break (zero-shot: the continuation is discarded, the value is delivered to the parent).

`recur()` and `done()` compile to `Perform(Continue(value))` and `Perform(Break(value))`.

### TryCatch as Handle/Perform

```
// tryCatch(body, recovery)
// Compiles to:
Handle(
  effect: Error,
  handler: {
    on Error(err) => evaluate recovery with err  // discard continuation
  },
  body
)
```

### Race as Handle/Perform

```
// race(a, b)
// Compiles to:
Handle(
  effect: FirstResult,
  handler: {
    on FirstResult(value) => return value  // discard continuation + cancel siblings
  },
  Parallel(
    Chain(a, Perform(FirstResult)),
    Chain(b, Perform(FirstResult))
  )
)
```

Whichever branch completes first performs `FirstResult`. The handler discards the continuation, which means the Parallel frame (and its other branch) must be cancelled. The implementation burden is cancellation semantics, not a new AST node.

### RAII (Bracket) as Handle/Perform

```
// A handler with dispose metadata, used in declare:
// declare({ wt: pipe(deriveBranch, createWorktree) }, body)
//
// Compiles to two nested handlers — one for the resource lifecycle,
// one for the variable binding:
Chain(
  pipe(deriveBranch, createWorktree),
  Handle(
    effect: Bracket,
    handler: {
      // On scope exit (success or error): run dispose on the stored value
      on_exit: dispose(stored_value)
    },
    Handle(
      effect: ReadVar("__0"),
      handler: { on ReadVar => resume(stored_value) },
      body
    )
  )
)
```

Variable binding (unrestricted: read many times) is separated from resource lifecycle (affine: disposed once). The Bracket handler manages disposal. The ReadVar handler manages lookup. They compose by nesting.

This separation matters if the execution model ever evolves to include lazy evaluation, detached forks, or continuation capture. Under those models, a ReadVar handler can be resumed after the Bracket handler has exited — and the Bracket handler will have already disposed the resource. The separation makes this a detectable error rather than a silent use-after-free.

Under the current strict sequential execution model, the two handlers always exit together, so the separation is invisible to the user. But it's architecturally clean.

## The scheduler's execution model

### StepResult: the core ADT

The scheduler evaluates frames by ticking them. Each tick returns one of:

```rust
pub enum StepResult {
    Done(Value),                       // frame completed, produced a value
    Suspend(Effect, FrameState),       // frame emitted an effect, yielded its state
    Dispatch(HandlerId, Value),        // frame hit an Invoke, needs external execution
    Error(WorkflowError),              // frame failed
}
```

`Suspend` is the new variant. When a `Perform` node is evaluated, it returns `Suspend(effect, state)`. The scheduler propagates this upward.

### Effect propagation (stack unwinding)

When the scheduler sees `Suspend`, it walks up the frame tree looking for a `Handle` frame that matches the effect type:

1. The Perform frame returns `Suspend(ReadVar("__0"), frame_state)`.
2. The parent (say, a Chain frame) receives `Suspend`. It doesn't handle effects. It freezes its own state and returns `Suspend` to its parent.
3. This continues up the tree until a Handle frame matching `ReadVar` is found.
4. The Handle frame receives the effect payload and the frozen frames between itself and the Perform site.

The frozen frames between the Perform and the Handle are the **continuation**. They're a snapshot of the execution path that was interrupted.

### Intermediate nodes don't handle effects

Chain, Parallel, Branch, ForEach are ignorant of effects. They only understand `Done`, `Suspend`, `Dispatch`, and `Error`. When a child returns `Suspend`:

- **Chain**: Freeze own state (remember which child completed, what the `rest` is). Return `Suspend` to parent.
- **Branch**: Same — freeze state, propagate.
- **ForEach**: Freeze state (which iterations completed, which suspended). Propagate.
- **Parallel**: This is the interesting case (see below).

### Parallel and suspension

When `Parallel(A, B)` is running and A returns `Suspend`:

1. The Parallel frame receives A's suspension.
2. B may still be executing (if the scheduler is processing children round-robin) or may have already completed.
3. The Parallel frame pauses. It freezes its state: A is suspended with `(effect, frame_state)`, B has its current status (Done, still running, etc.).
4. The Parallel frame returns `Suspend` to its parent, passing the effect upward.

When the Handle frame resumes the continuation:

1. The Parallel frame is restored.
2. A receives the resume value and transitions from Suspended to Done.
3. The Parallel frame waits for B to also complete (if it hasn't already).
4. Once both are Done, the Parallel frame returns `Done([result_a, result_b])`.

When the Handle frame **discards** the continuation (Race scenario):

1. The continuation (including the Parallel frame) is dropped.
2. Dropping the Parallel frame drops B's execution state.
3. If B had acquired resources, their disposal runs via Bracket handlers during teardown.

### Continuation representation

The continuation is the chain of frozen frames from the Perform site to the Handle site:

```rust
pub struct Continuation {
    /// Frozen frames, innermost first. Resuming pushes them
    /// back onto the scheduler's active frame stack.
    frames: Vec<FrozenFrame>,
}

impl Continuation {
    /// Resume by restoring frozen frames and injecting the value
    /// into the innermost frame.
    pub fn resume(self, scheduler: &mut Scheduler, value: Value) { ... }

    /// Discard by dropping all frozen frames, triggering cleanup.
    pub fn discard(self, scheduler: &mut Scheduler) { ... }
}
```

Rust's ownership model helps: dropping a Continuation drops the frozen frames, which can trigger RAII cleanup via the Drop trait on frame state that holds Bracket resources.

### Multi-shot continuations (loop)

Loop's Continue handler needs to re-enter the body. This looks like "resume the continuation multiple times," but it's simpler than general multi-shot continuations. The handler doesn't replay the old continuation — it starts a fresh execution of the body with the Continue value. The old continuation (from the Perform(Continue) site) is discarded.

So loop is actually zero-shot on Continue (discard the old continuation, start fresh) and zero-shot on Break (discard, exit). No multi-shot continuations are needed for any current or planned feature.

## Where Gemini's analysis was wrong

This analysis was informed by external feedback (Gemini) without source access. Corrections:

1. **`pipe` is already not an AST node.** `pipe(a, b, c)` compiles to nested Chain nodes in TypeScript. The TS-side sugar pattern that Gemini proposes is already how Barnum works. Handle/Perform extends this existing pattern to scope-based features.

2. **The flat HashMap "bottleneck" doesn't exist.** The scheduler is cooperative and single-threaded. There's no concurrent write contention. The persistent data structure recommendation is still valid for correctness (automatic scope cleanup), but the performance argument is moot.

3. **"De Bruijn naming scheme" is inaccurate.** De Bruijn indices are relative (distance from use site to binding site) and change under substitution. Monotonic counter IDs are absolute and stable. Different mechanism, different properties.

4. **The ReadVar dispatch mechanism is simpler than shown.** Gemini shows runtime string-matching dispatch (`if id == "__declare_0"`). In practice, each Handle frame holds one (id, value) pair. Lookup walks up the handler chain via parent pointers — structurally identical to how the current engine walks frames. No string matching at runtime; the frame tree encodes the scope chain.

## Environment as frame tree

Under Handle/Perform, the environment is implicit in the frame tree. Each Handle(ReadVar) frame holds one `(id, value)` pair. Looking up a variable means walking parent pointers to find the Handle that owns that id.

This IS a persistent immutable cons list, implemented via the frame tree structure. No separate data structure needed. When a Handle scope exits, its frame is removed — the binding disappears. When Parallel forks, both branches share the same parent chain — both can read the same variables without cloning.

For the current cooperative single-threaded scheduler, this is a simple parent-pointer walk. For a future multi-threaded scheduler, the frame tree would need to be made thread-safe (Arc-based parent pointers, or each thread gets its own frame stack with shared ancestry).

## Implementation path

Handle/Perform is the preferred direction. Fewest primitives, maximum extensibility, clean educational story.

### Phase 1: Handle/Perform in the scheduler

Add Handle and Perform to the flat table's action types. Implement the suspension/resumption mechanism in the Rust scheduler:

1. `FlatAction::Perform { effect }` — returns `StepResult::Suspend`
2. `FlatAction::Handle { effect_type, body }` — installs a handler frame
3. The scheduler's run loop gains a `Suspend` propagation path (analogous to the existing error propagation path)
4. Handle frames catch matching suspensions and execute handler logic
5. Continuation capture, resume, and discard

### Phase 2: Declare as the first sugar

Implement `declare` in TypeScript as sugar over Handle/Perform. This exercises the full mechanism:

- Perform(ReadVar) in the body
- Handle(ReadVar) around the body
- Eager evaluation of bindings before entering the Handle
- Both object form (concurrent via Parallel) and array form (sequential via nesting)

### Phase 3: Loop migration

Rewrite the existing LoopAction to compile to Handle/Perform. This exercises:

- Perform(Continue) and Perform(Break) replacing the current `recur()` / `done()` builtins
- Handle(LoopControl) replacing the current Loop frame kind
- Re-entry semantics (on Continue, start fresh body execution)
- The existing Loop tests validate that the migration is correct

### Phase 4: TryCatch, Timeout, Race, RAII

Each is new TypeScript surface sugar that compiles to Handle/Perform. The scheduler doesn't change. Each new feature exercises the mechanism in a new way:

- TryCatch: discard continuation on error
- Timeout: timer-based cancellation
- Race: first-completion with sibling cancellation
- RAII (Bracket): scope-exit cleanup, separated from variable binding

### What stays in the tree AST

The tree AST (what TypeScript produces) can keep specific node types for type checking and error messages: `DeclareAction`, `LoopAction`, `TryCatchAction`. These are desugared to Handle/Perform during flattening. Or the tree AST can use Handle/Perform directly, with TypeScript functions providing the type safety. This is a TS-side design choice that doesn't affect the scheduler.

The TS-vs-Rust boundary: currently, TypeScript does all rewriting (pipe → Chain, augment → Parallel+Identity+Merge, etc.) and the Rust flattener does layout + handler interning + step resolution. Adding Handle/Perform desugaring could happen on either side. Doing it in TypeScript keeps the flattener simple. Doing it in Rust keeps the tree AST closer to user intent.
