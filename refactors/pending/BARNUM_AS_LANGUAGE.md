# Barnum as a Language Runtime

Barnum is a compiler and interpreter for a small, first-order, concurrent workflow language with algebraic effects. This document maps its concepts to their equivalents in programming languages, compilers, and runtime systems.

## The two IRs

Barnum has two intermediate representations:

| Barnum | Compiler analogy |
|---|---|
| Tree AST (`Action` enum) | AST / HIR — what the frontend produces |
| Flat table (`FlatConfig`) | Bytecode / LIR — what the interpreter executes |

The TS combinators (`pipe`, `all`, `branch`, etc.) are the **surface syntax**. They produce the tree AST via builder functions — this is Barnum's "parser." `flatten()` is the **compiler**: it lowers the tree to a flat table with index-based references, interns handlers, and resolves step names. The engine is the **interpreter**: it walks the flat table and executes it.

This mirrors the standard compiler pipeline: source → AST → bytecode → VM. JVM, CPython, Lua, and Erlang/BEAM all work this way. The tree representation is good for construction and transformation; the flat representation is good for execution (cache-friendly, no pointer chasing, O(1) random access by ActionId).

## ActionId is an instruction pointer

In the flat table, `ActionId` is an index into a linear array of entries. The engine holds a cursor `(ActionId, Value)` and advances through the table. This is structurally identical to a bytecode VM's instruction pointer — `ActionId` is the IP, `Value` is the accumulator register.

The key difference: a bytecode VM steps one instruction at a time. Barnum's engine expands the cursor recursively until it hits Invoke leaves (the only instructions that "block"). A single `advance()` call can create an entire tree of frames. This is because Barnum has structured concurrency — All and ForEach fan out to multiple concurrent paths, all of which must reach Invoke before the engine yields control.

## Step is goto

`FlatAction::Step { target: ActionId }` is an unconditional jump. It redirects to another ActionId without creating a frame, modifying state, or consuming the value. It is `goto` in the purest sense — a raw control flow edge in the flat table's control flow graph.

Named steps in the tree AST (`Step("Cleanup")`) are symbolic labels that `flatten()` resolves to concrete ActionIds — exactly like assembler labels resolved to addresses during assembly. The flat table has no concept of "names"; only ActionIds.

This means the flat table is a **control flow graph** (CFG). Each ActionId is a node. Edges are:
- Chain: `first` → implicit (child slot), `rest` → explicit (`ActionId` field)
- Branch: N edges, one per case
- Loop: edge to body, edge back to self (on Continue)
- Step: unconditional edge to target
- All/ForEach: fan-out edges to children
- Handle: edge to body, edge to handler DAG
- Perform: edge up to matching Handle (effect bubbling)

The flattener produces this CFG from the tree AST. The engine traverses it.

## Frames are activation records

In a traditional language, a function call pushes an activation record (stack frame) onto the call stack. The frame holds local state, a return address, and (in some implementations) space for the return value.

Barnum's `Frame` is an activation record:

| Traditional frame | Barnum frame |
|---|---|
| Return address | `parent: Option<ParentRef>` |
| Local variables | `kind: FrameKind` (rest ActionId, results vec, handle state, etc.) |
| Stack pointer | Implicit — the frame is in a generational arena, not a stack |

The critical difference: **Barnum's frames form a tree, not a stack.** A linear call stack assumes sequential execution — each function calls at most one other function at a time. Barnum has All and ForEach, which create multiple concurrent children from a single parent. The frame "stack" fans out into a tree at every fan-out combinator.

This is **structured concurrency** in the frame topology. Every concurrent execution path is a branch of the frame tree, and every branch must complete before the parent can proceed. The tree structure guarantees no orphaned computations — when a parent is removed (e.g., Handle teardown on Discard), all its descendants are reachable and cancellable.

Erlang/BEAM has a similar tree topology (supervisor trees), but at the process level. Barnum has it at the frame level within a single execution.

The five frame kinds map to five control flow patterns:

| FrameKind | PL analogy |
|---|---|
| Chain | Tail call (trampoline) |
| All | Fork-join barrier |
| ForEach | Map (parallel array application) |
| Loop | Fixed-point iteration |
| Handle | Effect handler / exception frame |

## Chain is tail-call optimization

When Chain's child completes, the Chain frame removes itself and jumps to `rest` with the original parent. No frame accumulates. A chain of N sequential actions uses O(1) frames, not O(N).

This is exactly **tail-call elimination** (TCE). In a tail-recursive function, the caller's frame is replaced by the callee's frame before the call. Chain does the same thing — it replaces itself with the `rest` continuation. The trampoline pattern (remove frame → advance rest → new frame for rest's first child) prevents stack overflow for arbitrarily long sequential chains, just like TCE prevents stack overflow for arbitrarily deep tail recursion.

In Scheme, TCE is mandatory. In Barnum, it's a natural consequence of Chain's semantics — there's no state to preserve after the child completes, so the frame is unnecessary.

## Invoke is a syscall

Invoke is the boundary between the engine (pure state machine) and the external world. When the engine hits an Invoke, it produces a Dispatch — a `(TaskId, HandlerId, Value)` triple — and suspends.

This maps directly to the **syscall boundary** in an operating system. User-mode code does computation (control flow, data manipulation); when it needs I/O, it traps to the kernel. In Barnum, the engine does control flow; when it needs a value, it produces a Dispatch (the "trap") and suspends at an Invoke frame until the runtime provides a result (the "return from kernel").

The engine's purity — no I/O, no scheduling, no timers — is the equivalent of a user-mode process that can only interact with the world through syscalls. The runtime is the kernel.

The one exception: **builtins**. `HandlerKind::Builtin` variants (Identity, Tag, Merge, GetField, etc.) are executed inline by the Rust runtime without spawning a subprocess. These are the CPU's built-in instructions — `mov`, `lea`, field projection — as opposed to syscalls. They don't cross the engine/runtime boundary in the same way TypeScript handlers do.

## ParentRef is a continuation

When a child frame completes, it has a `parent: Option<ParentRef>` that tells the engine where to deliver the result. This is a **continuation** — a reified "what to do next" object.

The five ParentRef variants correspond to the five frame kinds:

| ParentRef variant | Continuation semantics |
|---|---|
| `Chain` | Direct: deliver value, trampoline to `rest` |
| `Loop` | Inspect: check Continue/Break, re-enter or exit |
| `All { child_index }` | Partial: fill one slot, join when all slots filled |
| `ForEach { child_index }` | Partial: same as All but for array elements |
| `Handle { side: Body }` | Body completed: deliver value to Handle's parent |
| `Handle { side: Handler }` | Handler completed: dispatch Resume/Discard/RestartBody |

In continuation-passing style (CPS), every function takes an explicit continuation argument. Barnum's `advance(action_id, value, parent)` is CPS: `parent` is the continuation. The `complete()` function is "invoke the continuation."

The `All`/`ForEach` partial continuations are **join points** — the parent's continuation is suspended until all partial results arrive. This is a barrier synchronization primitive, analogous to `pthread_barrier_wait` or Go's `sync.WaitGroup`.

## Handle/Perform is algebraic effects

Handle/Perform is the most important primitive in Barnum. It subsumes exception handling, cancellation, timeouts, races, variable binding, and loop control flow — all as instances of one mechanism.

### The mechanism

`Handle { effect_id, body, handler }` establishes an **effect handler frame**. `Perform { effect_id }` fires an effect. When a Perform executes inside the body, the engine walks up the frame tree looking for a Handle with a matching `effect_id`. When found:

1. The Handle frame is **suspended** — its body is frozen at the Perform site.
2. The handler DAG is invoked with `{ payload, state }` — the performed value and the Handle's accumulated state.
3. The handler produces one of three outcomes:

| HandlerOutput | Semantics | PL analogy |
|---|---|---|
| **Resume** { value, state_update } | Deliver value back to the Perform site. Body continues. | Koka's `resume(v)`, delimited continuation |
| **Discard** { value } | Tear down the entire body. Handle exits with value. | Exception catch, `longjmp` |
| **RestartBody** { value, state_update } | Tear down the body and re-enter from scratch with new input. | Loop re-entry, retry |

This is a **one-shot algebraic effect system**. It's one-shot because each Perform suspends the body exactly once — there's no multi-shot continuation (you can't resume the same Perform twice). Koka, Eff, and OCaml 5 all have similar one-shot restrictions by default.

### Effect bubbling

When a Perform fires, it bubbles up through ancestor frames until it finds a Handle with a matching `effect_id`. Non-matching Handles are skipped. This is **dynamic dispatch on the frame tree** — the same algorithm as exception propagation walking the call stack looking for a catch block.

If a Perform reaches the root without finding a matching Handle, the engine produces `UnhandledEffect` — analogous to an uncaught exception.

### What Handle/Perform subsumes

The power of Handle/Perform is that it replaces multiple special-purpose mechanisms with one general primitive:

| Feature | Compiled to |
|---|---|
| `tryCatch(body, recovery)` | Handle where handler runs recovery then Discards |
| `race(a, b, c)` | Handle around All; first Perform wins, body Discarded |
| `withTimeout(ms, body)` | Race between body and sleep, each tagging Ok/Err before Perform |
| `bind([a, b], ([x, y]) => ...)` | Nested Handles where handler reads from state and Resumes |
| `loop(body)` | Handle with RestartBody on Continue, Discard on Break |
| `scope(jump => body)` | Handle with RestartBody; jump fires Perform |

Every one of these is syntactic sugar over Handle/Perform. The engine implements one mechanism; the TS surface DSL provides ergonomic wrappers.

### Relation to PL theory

In Plotkin and Pretnar's formulation (2009), an algebraic effect handler is:

```
handle body with
  | return x → ...
  | op(x, k) → ... (k is the continuation)
```

Barnum's Handle is this, with the continuation reified as the three HandlerOutput variants. `Resume` invokes `k`. `Discard` discards `k`. `RestartBody` discards `k` and re-enters the body.

Koka uses the same model: `handle(body) { return(x) → ...; op(x) → resume(v) }`. Barnum's handler is a DAG (an AST subtree) rather than a function, so the "choice" of Resume/Discard/RestartBody is encoded as a Tag at the end of the handler DAG — the engine inspects the tag to determine which path to take. This is the first-order equivalent of Koka's `resume()` call.

### The stash

When an effect bubbles up and hits a **suspended** Handle (one whose handler is already running), the effect can't be dispatched immediately. The engine **stashes** it — queues it for later processing. After each `complete()`, the engine sweeps the stash repeatedly until no progress is made (fixed point).

This is **backpressure**. In a message-passing system, when a receiver is busy, messages queue up. The stash is Barnum's message queue. The sweep loop is the event loop draining the queue.

Deliveries (values being returned to parent frames) can also be stashed — if a delivery targets a frame whose ancestor Handle is suspended, the delivery is deferred. This ensures that a Handle's body doesn't advance while the handler is running, preventing reentrancy bugs.

### Handle state

Handle frames carry optional mutable state that persists across handler invocations. The handler receives `{ payload, state }` and can return a `state_update` with Resume or RestartBody. This is a **mutable cell scoped to the handler's lifetime** — the only form of mutable state in the engine.

`bind` uses this: the Handle's state holds the bound values (the All output tuple). Each Perform (VarRef access) reads from state via `GetField("state")` + `GetIndex(n)` and Resumes with the extracted value.

## Builtins are the ALU

`HandlerKind::Builtin` variants are executed inline by the Rust runtime — no subprocess, no serialization, no I/O:

| Builtin | Operation | CPU analogy |
|---|---|---|
| Identity | Pass through | `mov` (register copy) |
| Constant(v) | Produce fixed value | Immediate operand |
| Tag(k) | Wrap as `{ kind: k, value: v }` | Constructor / tag bits |
| GetField(f) | Read object field | `lea` / field projection |
| GetIndex(n) | Read array element | Indexed load |
| Pick(keys) | Select object fields | Struct projection |
| Merge | Merge tuple of objects | Struct concatenation |
| Flatten | Flatten nested array | Memcpy |
| Drop | Discard value | `/dev/null` |
| CollectSome | Filter array, keep Some values | Filter + compact |

Without builtins, every trivial operation (extract a field, wrap in a tag) would require a TypeScript subprocess round trip. Builtins are the **ALU** — the engine can compute, not just route.

The distinction between builtins and TypeScript handlers mirrors the distinction between CPU instructions and system calls. Builtins execute inside the engine; TypeScript handlers cross the boundary.

## HOAS: closures at construction time

Barnum's runtime is first-order — the flat table is a static CFG with no closures or function values. But the TS surface DSL uses **higher-order abstract syntax (HOAS)** to provide closure-like scoping at AST construction time.

The pattern: a combinator takes a callback that receives **effect tokens** (Perform nodes with a specific EffectId), and returns an AST subtree that uses those tokens. The callback runs once, at construction time, to build the AST. It is not stored in the AST and does not exist at runtime.

```typescript
// tryCatch: callback receives throwError token
tryCatch(
  (throwError) => pipe(step1, handler.unwrapOr(throwError), step2),
  recovery,
)

// bind: callback receives VarRef tokens
bind([computeA(), computeB()], ([a, b]) =>
  pipe(a.then(transform), b.then(otherTransform)),
)

// loop (future closure form): callback receives recur/done tokens
loop(({ recur, done }) =>
  pipe(body, branch({ Continue: recur(), Break: done() })),
)
```

Each callback scopes its tokens to a specific EffectId, generated by `allocateEffectId()`. The EffectId is a gensym — a fresh integer that ties each Perform to its enclosing Handle. This is how lexical scoping works in HOAS: the "variable" (EffectId) is bound by the callback's closure over the gensym, and the resulting AST is fully resolved — no free variables.

This is the same technique used in:
- **Quoted DSLs** (MetaOCaml, Haskell Template Haskell): build code at compile time using host-language closures.
- **Tagless-final style**: combinators take callbacks that produce AST fragments.
- **PHOAS** (parametric HOAS): the "variable" type is abstract, preventing ill-scoped references by construction.

The key property: **the flat table has no closures, but the TS construction code does.** The closures exist only during AST construction. By the time the engine sees the flat table, all scoping has been resolved to integer EffectIds. This is the same phase distinction as macro expansion — macros are higher-order, but the expanded code is first-order.

## The engine is a cooperative scheduler

The engine does not preempt. It runs `advance()` synchronously until all paths are suspended at Invoke leaves, then yields the accumulated dispatches. External completions arrive via `complete()`, which may call `advance()` again, producing more dispatches.

This is a **cooperative multitasking** scheduler. Each Invoke is a yield point. The engine runs until it yields, processes one external event, runs until it yields again, and so on. There is no preemption, no timeslicing, no thread scheduling.

The pattern is identical to:
- **Node.js event loop**: run synchronous code until it's done, then process the next event from the queue.
- **Erlang/BEAM scheduler**: run a process until it yields (at a receive or after N reductions), then schedule the next process.
- **Async/await runtimes** (Tokio, Go scheduler): run a future/goroutine until it hits an await point, then park it and run another.

The difference: those systems multiplex many tasks on shared threads. Barnum's engine runs a single workflow — all concurrency is *within* the workflow (All/ForEach), not *between* workflows. Multiple workflows would each have their own engine instance.

## No runtime closures, no higher-order functions

Barnum's runtime is **first-order**. Actions are data (an enum), not functions. You can't pass an action as input to a handler, and handlers can't return actions. There's no lambda, no closure, no partial application in the flat table.

This is a deliberate constraint. First-order programs are fully inspectable — the flat table is a complete, static description of all possible control flow. You can analyze, optimize, and visualize the entire workflow without executing it. Higher-order functions would make the control flow dynamic and opaque.

The closest analogy is **SQL** — a declarative, first-order language where the query plan is fully determined before execution. Or **shader languages** (GLSL, HLSL) — first-order, fully inspectable, compiled to a flat instruction stream for hardware execution.

The TS surface DSL uses HOAS (see above) to provide closure-like scoping at construction time. The abstraction lives in the **metaprogram** (TypeScript), not in the **object program** (Barnum). This is the same split as C++ templates (compile-time abstraction) vs runtime polymorphism, or Lisp macros vs runtime functions.

## Handler interning is a constant pool

`flatten()` deduplicates identical handlers and assigns each unique handler a `HandlerId`. The engine's handler table is a **constant pool** — a side table of deduplicated constants referenced by index from the instruction stream.

JVM class files have a constant pool for strings, class references, and method descriptors. Lua bytecodes reference a constant table. Barnum's handler table serves the same purpose: avoid duplicating handler metadata (module path, function name, schemas) across every Invoke that uses the same handler.

## Features Barnum has gained

These were identified as missing in an earlier version of this document and have since been implemented.

### Variables and let-bindings (via bind)

`bind` and `bindInput` provide named intermediate values. `bind([a, b], ([x, y]) => body)` evaluates bindings concurrently, then makes them available as VarRefs throughout the body. A VarRef is a Perform node — reading a variable is an effect that the enclosing Handle intercepts and resumes with the stored value.

This is **Provide/Consume** (dynamic scoping over the frame tree), implemented via Handle/Perform. Each binding gets its own EffectId. The Handle's state stores the computed values. Each VarRef Perform reads from state and Resumes. The mechanism is general — it works across fan-out boundaries, step boundaries, and arbitrary nesting.

The functional programming analogy: `bind` is Haskell's `do`-notation — it desugars to monadic bind, but at the AST level rather than the value level. Point-free pipelines are still the default for simple cases; `bind` enables pointed (named) style when data plumbing becomes complex.

### Builtins (the ALU)

Identity, Constant, Tag, Merge, Flatten, GetField, GetIndex, Pick, Drop, and CollectSome all execute inline in Rust. See "Builtins are the ALU" above.

### Typed error handling (via tryCatch)

`tryCatch(body, recovery)` provides typed error handling via Handle/Perform. The body callback receives a `throwError` token (a Perform node typed as `TypedAction<TError, never>`). When the token fires, the Handle runs recovery with the error payload and Discards the body.

This handles **type-level errors** — values explicitly thrown via the token. If a handler panics or throws a JavaScript exception, that's a different failure mode (runtime crash, not typed error). Analogous to Rust's `Result` vs `panic!`.

The TS type system tracks the error type `TError` through the pipeline. `handler.unwrapOr(throwError)` extracts `Result<TValue, TError>` and either passes through Ok or fires the throw token with the Err payload. This is Rust's `?` operator — early return on error, with the error type statically known.

### Option and Result combinators

Full Rust-style `Option<T>` and `Result<TValue, TError>` namespaces with map, andThen, unwrapOr, flatten, filter, toOption, transpose, etc. All desugar to Branch + existing builtins — no new engine primitives. Postfix methods (`.mapOption()`, `.unwrapOr()`) are gated by `this` parameter constraints.

## Features Barnum still lacks

### Recursion and function calls

Step + Chain IS function calls. When you write `pipe(step("Validate"), processResult)`:

1. Step jumps to Validate's body (the function entry point)
2. Validate runs and produces a value
3. The Chain trampolines to `processResult` with the value (the return)

The Chain's `rest` field is the return address. Multiple call sites calling the same step each have their own Chain frame with a different `rest` — same function body, different return points. This is mechanically identical to a call stack: push return address (create Chain frame with rest), jump to function (Step to target ActionId), execute, pop return address (Chain trampolines to rest).

The one thing missing is parameterized calls — you can't pass arguments to a step. The step receives whatever value was flowing through the pipeline. In practice this isn't a limitation because pipeline values ARE the arguments.

### Mutable shared state

Barnum has no shared state between concurrent branches. All children each receive a clone of the input; they cannot communicate, share mutable state, or observe each other's progress.

**Does it matter?** This is a feature, not a limitation. Shared mutable state in concurrent systems causes races, deadlocks, and nondeterminism. Barnum's fork-join model guarantees deterministic ordering (results are collected in array order, not completion order) and eliminates data races by construction.

If a workflow needs to coordinate between branches, the answer is: don't use All. Use Chain to sequence the dependent parts. Or use a handler that writes to an external database and another that reads from it — the coordination happens outside Barnum, mediated by external state.

Handle state is the one exception — it's mutable and persists across handler invocations. But it's scoped to a single Handle frame and only accessible from the handler DAG, not from concurrent body branches. It's a controlled escape hatch, not shared mutable state.

**Verdict:** Doesn't matter. Deliberate constraint for correctness.

### Dynamic dispatch / polymorphism

Branch provides static dispatch: the cases are fixed at compile time, and the engine looks up the matching case by string key. There's no dynamic dispatch — you can't choose which action to run based on a runtime-computed action reference.

**Does it matter?** Workflow branching is almost always on a known, finite set of variants (success/failure, request type, user role). Open-ended dispatch (where the set of cases isn't known at compile time) would undermine the static inspectability of the flat table. If the target ActionId is computed at runtime, you can't analyze the workflow's possible paths without executing it.

**Verdict:** Doesn't matter. Static dispatch covers all practical workflow branching patterns.

### Conditional expressions (if/else without tagged unions)

Branch requires the input to be a tagged union (`{ kind: "...", ... }`). There's no `if (condition)` that evaluates a boolean expression. To branch on a boolean, you'd need a handler that converts `true`/`false` to `{ kind: "True" }` / `{ kind: "False" }`, then Branch on that.

**Does it matter?** It's clunky for simple boolean conditions. But tagged unions are strictly more general than booleans — they carry data per variant, enable exhaustiveness checking, and compose with the type system. The TS surface DSL could provide an `ifElse(condition, thenAction, elseAction)` combinator that desugars to `pipe(condition, branch({ True: thenAction, False: elseAction }))`.

**Verdict:** Minor ergonomics issue. Solvable with a TS combinator, no engine changes.

### General Provide/Consume (context, reader)

`bind` implements Provide/Consume for computed values — run some actions, make results available downstream via VarRefs. But there's no general mechanism for:
- **Static context** (API keys, tenant config) that doesn't come from a pipeline computation.
- **Cross-step scoping** — a value provided in one step available in steps it jumps to.

`bind` covers the common case (capture intermediate results). A general `provide(name, value, body)` / `consume(name)` mechanism would cover the remaining cases. The engine infrastructure exists — Handle/Perform already does scope-tree walks. A Provide/Consume combinator would be a thin layer over Handle/Perform where the handler always Resumes with the provided value.

**Verdict:** `bind` covers 90% of use cases. General Provide/Consume is a nice-to-have, not a blocker.

### Durability and persistence

All state is in-memory. If the process dies, in-flight workflows are lost. There's no checkpointing, no replay, no resume-after-crash.

The architecture is well-suited for durability — the engine is a pure state machine, so checkpointing is "serialize engine state at an effect boundary." Handle/Perform provides natural checkpoint boundaries (every effect is an explicit, typed yield point). But nothing is implemented.

**Verdict:** Critical for production use. The single biggest gap between Barnum and a production workflow engine.

## What Barnum is not

- **Not Turing-complete on its own.** Without handlers, the engine can only loop forever or terminate immediately. Handlers provide all computation. The engine is a scheduler, not a computer. (Builtins add trivial data transformations but not general computation.)
- **Not a process calculus.** No communication between concurrent branches (All children can't send messages to each other). Concurrency is fork-join only.
- **Not a dataflow language.** Data flows sequentially through chains and fans out through All, but there are no feedback loops or streaming — each value is produced once and consumed once.
- **An algebraic effect system.** Handle/Perform is a genuine one-shot algebraic effect handler in the PL theory sense. The earlier characterization of Barnum as "not a monad stack" with "hardcoded interpreter cases" is no longer accurate. Handle/Perform is a single, general mechanism. tryCatch, race, withTimeout, bind, and loop are all compiled to Handle/Perform — they are not special cases in the engine. The engine implements one effect handler mechanism; everything else is sugar.
