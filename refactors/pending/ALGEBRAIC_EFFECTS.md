# Algebraic Effects as the Unifying Primitive

## The diagnosis

Barnum's current AST is grounded in Cartesian Monoidal Categories: `Chain` (composition), `Parallel` (tensor product), `Branch` (coproduct elimination), `ForEach` (functorial map), `Invoke` (external computation). This is a point-free, dataflow topology. Data flows through the graph without names.

The prop drilling problem is the fundamental weakness of point-free topologies. When step C needs the output of step A and step A's output isn't on the direct pipeline path to step C, you must manually route it (augment, tap, pick). This isn't a missing feature. It's a structural property of point-free composition.

The proposed `declare` + `VarRef` introduces the Lambda Calculus: named bindings, lexical scope, an environment. This is point-ful evaluation bolted onto a Cartesian structure. The two models coexist but don't unify.

## The unification: Handle/Perform

`declare`, `tryCatch`, `loop`, `withTimeout`, and any future scope-creating combinator are all instances of algebraic effects. The two primitives:

- **`Handle`**: Install a scoped effect handler around a body. When the body (or anything nested inside it) performs a matching effect, control transfers to the handler. The handler receives the effect's payload and a continuation representing "the rest of the computation."
- **`Perform`**: Suspend execution and yield a value to the nearest enclosing `Handle` that matches.

### Declare as Handle/Perform

```
declare({ x: computeX }, ({ x }) => body_using_x)

// Compiles to:
Handle(
  handlers: {
    ReadVar(id) => if id == "__declare_0" then resume(evaluate(computeX))
                   else re-perform(ReadVar(id))  // propagate to outer scope
  },
  body: body_with_VarRefs_replaced_by_Perform(ReadVar("__declare_0"))
)
```

When the body hits a VarRef, it performs `ReadVar(id)`. The nearest Handle that knows about that id looks up (or evaluates) the binding and resumes the continuation with the value.

### Loop as Handle/Perform

```
loop(body)

// Compiles to:
Handle(
  handlers: {
    Continue(value) => restart body with value  // re-enter the handler's body
    Break(value) => return value                // discard continuation, exit handler
  },
  body
)
```

The handler intercepts Continue by re-entering its own body (capturing the re-entry point). It intercepts Break by discarding the continuation and delivering the value to the parent. This is what the current LoopAction frame already does, just expressed as a general mechanism.

### TryCatch as Handle/Perform

```
tryCatch(body, recovery)

// Compiles to:
Handle(
  handlers: {
    Error(err) => evaluate recovery with err  // discard continuation, run recovery
  },
  body
)
```

The handler intercepts errors. On error, it discards the body's continuation and runs the recovery action instead.

### Timeout as Handle/Perform

```
withTimeout(duration, body)

// Compiles to:
Handle(
  handlers: {
    Timeout() => cancel body, propagate timeout error
  },
  Chain(
    Parallel(body, timer(duration)),
    // whichever completes first triggers the handler
  )
)
```

Timer completion performs a Timeout effect. The handler cancels the body and propagates.

## The continuation question

Handle/Perform requires the scheduler to reify continuations. When a Perform suspends execution, the handler receives:
1. The effect's payload (a variable ID, an error value, a Continue value)
2. A continuation — "the rest of the computation" from the Perform site to the Handle site

The handler decides what to do with the continuation:
- **Resume once** (declare: look up value, resume with it)
- **Discard** (tryCatch on error: don't resume, run recovery instead)
- **Resume repeatedly** (loop on Continue: re-enter body, which means invoking the handler again)
- **Resume zero times** (timeout: cancel everything)

The current engine doesn't have explicit continuations. Each frame has a `parent: Option<ParentRef>` that implicitly encodes "what to do next." The continuation is the frame tree from the current frame up to the root.

Implementing general Handle/Perform requires making this implicit continuation explicit and manipulable. The scheduler must be able to:
- Capture a portion of the frame tree (from Perform site to Handle site)
- Invoke it (resume the computation)
- Discard it (cancel all frames in the captured portion)
- Copy it (for multi-shot continuations, if ever needed)

This is a significant change to the frame model. Each specific scope type (declare, loop, tryCatch) can be implemented with specific frame logic that doesn't require general continuation capture. Handle/Perform generalizes this at the cost of a more complex frame model.

### Two-level architecture

A pragmatic approach: keep specific AST nodes in the tree AST (what TypeScript produces) and compile to Handle/Perform in the flat table (what Rust executes).

- **Tree AST**: `DeclareAction`, `LoopAction`, `TryCatchAction`, etc. Specific nodes with specific types, specific error messages, specific validation. The TypeScript surface API produces these.
- **Flat table**: `HandleAction`, `PerformAction`. The flattener desugars specific nodes into Handle/Perform pairs. The Rust scheduler implements one general mechanism.

This is how compilers work. `for`, `while`, `if/else`, `try/catch` exist in the AST. They compile to `branch`, `jump`, `compare` in the instruction set. The AST preserves intent and enables validation. The instruction set is minimal and general.

Benefits:
- TypeScript type checking works on specific nodes (DeclareAction carries TIn, TOut, the binding types)
- Error messages reference user concepts ("variable '__declare_0' not found" vs "unhandled effect 'ReadVar'")
- The scheduler implements one frame model (Handle) instead of N frame kinds
- New scope types require only: new TS surface function + new AST node + new flattener case. No scheduler changes.

Cost:
- The flattener becomes a real compiler pass, not just a tree-to-table layout
- The scheduler must implement general continuation management
- Debugging the flat table requires mapping back to tree AST concepts

## RAII: separate concern from variable binding

The LET_BINDINGS.md proposal ties resource disposal to `declare`'s scope exit. This conflates two type-theoretic concepts:

- **Unrestricted variables**: Read zero or more times. No lifecycle constraint. A VarRef is pure data lookup.
- **Affine resources**: Used at most once, must be disposed. A resource has a lifecycle that must be managed.

A `declare`-bound variable that happens to come from a disposable handler is both: a variable (read many times) and a resource (disposed once). The current proposal handles both with one mechanism (Declare scope exit runs dispose). This works under strict sequential execution where lexical scope matches temporal lifetime.

It breaks if:
- **Lazy evaluation**: A VarRef is forced after the Declare scope exits. The value was disposed. Use-after-free.
- **Detached forks**: A VarRef is passed to a background process. The Declare scope exits and disposes. The background process holds a dangling reference.
- **Continuation capture**: A continuation that includes a VarRef is captured and resumed after the Declare scope exits.

Under Handle/Perform, resource management is a separate effect:

```
// Variable binding: ReadVar effect
Handle(
  { ReadVar(id) => resume(lookup(id)) },
  body
)

// Resource management: Bracket effect
Handle(
  {
    Acquire(action) => evaluate action, track resource, resume(resource)
    Release() => dispose all tracked resources (reverse order)
    // Release fires automatically on handler scope exit (success or error)
  },
  body
)
```

The two can be composed:

```
// User writes: declare({ wt: createWorktree }, body)
// Where createWorktree has dispose metadata

// Compiles to nested handlers:
Handle(Bracket, {
  Handle(ReadVar, {
    body  // VarRefs perform ReadVar; resource cleanup on Bracket exit
  })
})
```

The user writes one thing. The compiler generates two nested handlers: one for the resource lifecycle (affine), one for the variable binding (unrestricted). The concerns are separated at the semantic level.

### Current execution model makes the conflation safe

The current Barnum scheduler is cooperative and single-threaded. There are no detached forks, no lazy evaluation, no continuation capture. Lexical scope perfectly matches temporal lifetime. Under these constraints, tying disposal to Declare scope exit is correct.

The risk is: if the execution model evolves (lazy bindings, background tasks, durable execution with resume), the conflation becomes a bug. Separating the concerns now (even if the implementation happens to interleave them) makes the migration path clear.

## Environment: persistent data structure, not flat HashMap

The LET_BINDINGS.md proposal uses a single `HashMap<DeclareId, Value>` for the environment. Under Handle/Perform, the environment is an effect handler's local state. Each Handle for ReadVar maintains its own binding.

A persistent immutable data structure (cons list of `(id, value)` pairs, or a persistent hash map like an HAMT) has advantages:

- **Automatic scope cleanup**: When a Declare scope exits, the extended environment is simply no longer referenced. No explicit removal needed. The garbage collector (or Rust ownership) handles it.
- **No mutation**: Parallel branches each receive the same environment reference. No cloning, no locking. Reads are safe without synchronization because the data is immutable.
- **Thread safety for free**: If the scheduler ever goes multi-threaded, the persistent data structure works without changes.

The flat HashMap requires explicit cleanup (remove bindings on scope exit) and explicit cloning or locking for parallel branches. The persistent structure avoids both.

Lookup cost: O(n) for a cons list (where n is the number of bindings in scope), O(log n) for an HAMT. Given that typical workflows have tens of bindings, not thousands, the cons list is fine. The constant factor is lower than a HashMap for small n.

Under Handle/Perform, the environment is even simpler: each Handle frame for ReadVar holds exactly one `(id, value)` pair. Lookup walks up the handler chain (parent pointers). This IS a cons list, implemented via the frame tree itself.

## Race: derived from cancellation semantics

Race is not a primitive. It's `Parallel` + `Handle` + cancellation:

```
race(a, b)

// Compiles to:
Handle(
  { FirstResult(value) => return value  // discard continuation, cancelling the other branch },
  Parallel(
    Chain(a, Perform(FirstResult)),
    Chain(b, Perform(FirstResult))
  )
)
```

Whichever branch completes first performs `FirstResult`. The handler takes the value and discards the continuation, which means the entire Parallel subgraph (including the other branch) must be cancelled.

The implementation burden is cancellation: the scheduler must traverse the orphaned branch's frame subtree and tear it down, running dispose handlers for any acquired resources. If the scheduler's frame-drop logic is sound, Race falls out for free.

If Race is added as a separate AST node, it masks a deficiency in cancellation semantics. The right approach: ensure cancellation works correctly, then build Race in the TypeScript surface layer.

## Implementation path

### Option A: Handle/Perform first (theoretical purity)

Build the general mechanism. All scope types are TypeScript sugar. The scheduler implements one frame kind.

Cost: continuation management in the scheduler is hard. Multi-shot continuations (loop), single-shot (declare), zero-shot (tryCatch on error) all require different handling. The scheduler must track continuation state, handle cleanup on discard, and prevent misuse (resuming a consumed continuation).

Benefit: future scope types require zero scheduler changes. The AST stays minimal. The frame model is general and proven correct once.

### Option B: Specific frame kinds first (pragmatic)

Build DeclareAction, then TryCatchAction, then TimeoutAction. Each is a specific frame kind in the scheduler. Refactor to Handle/Perform later if the pattern proves out.

Cost: N frame kinds in the scheduler, each with its own enter/exit/error logic. When N gets large, the scheduler's match arms multiply. Adding a new scope type requires scheduler changes.

Benefit: each step is small, testable, and delivers user value immediately. The scheduler remains simple. Type safety and error messages are specific.

### Option C: Two-level architecture (recommended)

Tree AST has specific nodes. Flat table uses Handle/Perform. The flattener compiles specific nodes to Handle/Perform.

Cost: the flattener becomes a real compiler pass. The scheduler implements Handle/Perform (same cost as Option A).

Benefit: TypeScript surface stays clean and well-typed. Scheduler is general. New scope types require: TS function + AST node + flattener case. No scheduler changes.

This is the standard compiler architecture: rich frontend AST, minimal backend IR.

### What to build first regardless of option

No matter which path, the first step is the same: implement a single-binding Declare that adds one entry to the environment and executes a body. Under Option A, this is Handle(ReadVar, body). Under Option B, this is a Declare frame kind. Under Option C, this is a DeclareAction in the tree AST that the flattener lowers to Handle(ReadVar, body) in the flat table.

The implementation of the first scope type is the same work regardless. The question is whether the scheduler's frame model is general (Handle) or specific (Declare). That decision can be deferred to the point where we implement the second scope type (tryCatch), because that's when the generalization pays off.
