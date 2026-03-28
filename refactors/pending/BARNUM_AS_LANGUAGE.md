# Barnum as a Language Runtime

Barnum is a compiler and interpreter for a small, first-order, concurrent workflow language. This document maps its concepts to their equivalents in programming languages, compilers, and runtime systems.

## The two IRs

Barnum has two intermediate representations:

| Barnum | Compiler analogy |
|---|---|
| Tree AST (`Action` enum) | AST / HIR — what the frontend produces |
| Flat table (`FlatConfig`) | Bytecode / LIR — what the interpreter executes |

The TS combinators (`pipe`, `parallel`, `branch`, etc.) are the **surface syntax**. They produce the tree AST via builder functions — this is Barnum's "parser." `flatten()` is the **compiler**: it lowers the tree to a flat table with index-based references, interns handlers, and resolves step names. The engine is the **interpreter**: it walks the flat table and executes it.

This mirrors the standard compiler pipeline: source → AST → bytecode → VM. JVM, CPython, Lua, and Erlang/BEAM all work this way. The tree representation is good for construction and transformation; the flat representation is good for execution (cache-friendly, no pointer chasing, O(1) random access by ActionId).

## ActionId is an instruction pointer

In the flat table, `ActionId` is an index into a linear array of entries. The engine holds a cursor `(ActionId, Value)` and advances through the table. This is structurally identical to a bytecode VM's instruction pointer — `ActionId` is the IP, `Value` is the accumulator register.

The key difference: a bytecode VM steps one instruction at a time. Barnum's engine expands the cursor recursively until it hits Invoke leaves (the only instructions that "block"). A single `advance()` call can create an entire tree of frames. This is because Barnum has structured concurrency — Parallel and ForEach fan out to multiple concurrent paths, all of which must reach Invoke before the engine yields control.

## Step is goto

`FlatAction::Step { target: ActionId }` is an unconditional jump. It redirects to another ActionId without creating a frame, modifying state, or consuming the value. It is `goto` in the purest sense — a raw control flow edge in the flat table's control flow graph.

Named steps in the tree AST (`Step("Cleanup")`) are symbolic labels that `flatten()` resolves to concrete ActionIds — exactly like assembler labels resolved to addresses during assembly. The flat table has no concept of "names"; only ActionIds.

This means the flat table is a **control flow graph** (CFG). Each ActionId is a node. Edges are:
- Chain: `first` → implicit (child slot), `rest` → explicit (`ActionId` field)
- Branch: N edges, one per case
- Loop: edge to body, edge back to self (on Continue)
- Step: unconditional edge to target
- Parallel/ForEach: fan-out edges to children

The flattener produces this CFG from the tree AST. The engine traverses it.

## Frames are activation records

In a traditional language, a function call pushes an activation record (stack frame) onto the call stack. The frame holds local state, a return address, and (in some implementations) space for the return value.

Barnum's `Frame` is an activation record:

| Traditional frame | Barnum frame |
|---|---|
| Return address | `parent: Option<ParentRef>` |
| Local variables | `kind: FrameKind` (rest ActionId, results vec, etc.) |
| Stack pointer | Implicit — the frame is in a HashMap, not a stack |

The critical difference: **Barnum's frames form a tree, not a stack.** A linear call stack assumes sequential execution — each function calls at most one other function at a time. Barnum has Parallel and ForEach, which create multiple concurrent children from a single parent. The frame "stack" fans out into a tree at every fan-out combinator.

This is **structured concurrency** in the frame topology. Every concurrent execution path is a branch of the frame tree, and every branch must complete before the parent can proceed. The tree structure guarantees no orphaned computations — when a parent is removed (e.g., error cancellation), all its descendants are reachable and cancellable.

Erlang/BEAM has a similar tree topology (supervisor trees), but at the process level. Barnum has it at the frame level within a single execution.

## Chain is tail-call optimization

When Chain's child completes, the Chain frame removes itself and jumps to `rest` with the original parent. No frame accumulates. A chain of N sequential actions uses O(1) frames, not O(N).

This is exactly **tail-call elimination** (TCE). In a tail-recursive function, the caller's frame is replaced by the callee's frame before the call. Chain does the same thing — it replaces itself with the `rest` continuation. The trampoline pattern (remove frame → advance rest → new frame for rest's first child) prevents stack overflow for arbitrarily long sequential chains, just like TCE prevents stack overflow for arbitrarily deep tail recursion.

In Scheme, TCE is mandatory. In Barnum, it's a natural consequence of Chain's semantics — there's no state to preserve after the child completes, so the frame is unnecessary.

## Invoke is a syscall

Invoke is the only `FlatAction` that produces a value from the outside world. Every other action is pure control flow — routing, forking, joining, looping, branching. Invoke is the boundary between the engine (pure state machine) and the external runtime (I/O, scheduling, handler execution).

This maps directly to the **syscall boundary** in an operating system. User-mode code does computation (control flow, data manipulation); when it needs I/O, it traps to the kernel. In Barnum, the engine does control flow; when it needs a value, it produces a Dispatch (the "trap") and suspends at an Invoke frame until the runtime provides a result (the "return from kernel").

The engine's purity — no I/O, no scheduling, no timers — is the equivalent of a user-mode process that can only interact with the world through syscalls. The runtime is the kernel.

## ParentRef is a continuation

When a child frame completes, it has a `parent: Option<ParentRef>` that tells the engine where to deliver the result. This is a **continuation** — a reified "what to do next" object.

`ParentRef::SingleChild` is a direct continuation: deliver the value to the parent frame, which will do one thing with it (Chain trampolines, Loop inspects, Attempt wraps).

`ParentRef::IndexedChild` is a **partial continuation**: deliver the value to slot `child_index` of the parent's results vec. The parent doesn't continue until *all* slots are filled. This is a join point — the parent's continuation is suspended until all partial results arrive.

In continuation-passing style (CPS), every function takes an explicit continuation argument. Barnum's `advance(action_id, value, parent)` is CPS: `parent` is the continuation. The `complete(parent, value)` function is "invoke the continuation."

## Attempt is exception handling

`Attempt` wraps its child's result in `Ok`/`Err`, catching errors that would otherwise propagate up the frame tree. This is `try/catch` — an error boundary.

Error propagation in `error(parent, error)` walks up the frame tree until it finds an Attempt frame or reaches the root. This is **stack unwinding** — the same mechanism used by C++, Java, and Rust for exception/panic propagation. Each frame on the path is cleaned up (removed), and fan-out frames cancel their siblings.

The difference from most languages: Barnum's error propagation must cancel sibling branches in Parallel/ForEach frames. In a linear call stack, there are no siblings to cancel. Structured concurrency adds a dimension to unwinding — it's not just upward, it's also lateral (cancel siblings before propagating to the parent).

## Speculation: Result as the fundamental type

The current engine has two channels: values flow down through `complete`, errors flow up through `error`. `Attempt` is the bridge — it catches errors from the error channel and lifts them into the value channel as `{ kind: "Err", error }`.

An alternative design: **there is no error channel.** Every action produces a `Result` — `{ kind: "Ok", value }` or `{ kind: "Err", error }`. Chain is monadic bind: it auto-unwraps `Ok` and short-circuits on `Err`. Attempt means "stop the auto-unwrapping — give me the raw Result."

### How it would work

In this model, Chain's `complete_single` changes from "always advance to rest" to "check the value: if Ok, unwrap and advance; if Err, propagate upward." The "propagate upward" case is: call `complete(parent, the_err_result)`, which hits the parent chain, which also sees Err, which also propagates — all the way up until something catches it. That "something" is Attempt (which passes the raw Result through without unwrapping) or Root (which terminates the engine).

This upward Err propagation IS the current `error()` function. The two-channel implementation is an optimization of the Result model: instead of checking Ok/Err at every Chain, it uses a separate code path (`error`) that skips directly to the catching frame. The semantics are identical; the implementation avoids redundant Ok checks on the happy path.

### Handlers never see Results

A handler returns a value. The framework wraps it in Ok. A handler throws. The framework wraps the thrown error in Err. From JavaScript, Results are invisible — you write normal code, and the monadic machinery is internal to the engine.

This is how `const(42)` works: the handler ignores its input and returns `42`. The engine wraps this as `Ok(42)`. Chain auto-unwraps it. The next handler receives `42`. Nobody outside the engine ever sees the Result wrapper. The constructor "includes the unwrap" because Chain is monadic bind.

The same applies to every handler. `fetchUser()` returns a user object → the engine sees `Ok(user)`. `fetchUser()` throws a network error → the engine sees `Err("network error")`. Chain unwraps Ok and short-circuits Err. The handler author writes the same code either way.

### Timeouts and typed errors

In the Result model, a timeout is just an Err variant. An auth failure is another Err variant. They're all data in the value channel:

```
attempt(
  potentiallySlowAction
).then(
  branch({
    Ok: handleSuccess,
    Err: branch({
      Timeout: retry,
      RateLimit: backoff,
      AuthError: abort,
    })
  })
)
```

This actually works in the current design too — Attempt catches the error and wraps it as `{ kind: "Err", error }`, then you branch on the result. But in the Result model, it's more natural: errors are *always* data, and Attempt is just "stop auto-unwrapping" rather than "intercept a separate channel."

The current design requires the error channel to carry untyped `String`s — there's no way to propagate structured error types through `error()`. The Result model makes typed errors trivial: the Err variant is just a Value, with whatever structure the handler put there. Branch on `error.kind` the same way you'd branch on any other discriminated union.

### Which is more fundamental?

Result is more fundamental. You can implement exceptions from Results (Rust's `?` operator is exactly this: monadic bind over `Result`, short-circuiting on `Err`). You can't implement typed Results from exceptions — exceptions flow through an implicit untyped channel.

The current engine is operationally equivalent to the Result model. It's an optimization: two code paths (`complete` + `error`) instead of one code path that checks Ok/Err at every step. This avoids N conditional checks on the happy path (one per Chain frame) at the cost of a separate error propagation function.

The "Not a monad stack" characterization below is slightly misleading in light of this. Chain *is* monadic bind over an implicit Result type — the engine just implements the bind as two separate functions instead of one function with a conditional. If we ever implement typed errors or general Provide/Consume, the Result framing becomes harder to avoid.

### Implication for the engine

No immediate changes. The current two-channel implementation is correct and performant. But the mental model should be: **every action produces a Result; Chain is bind; Attempt is catch.** The `error()` function is the optimized Err-propagation path of bind. When we add typed errors (DEFERRED_FEATURES.md), the Result framing will guide the design — error types are just the Err variant of the Result, dispatched by Branch like any other discriminated union.

## The engine is a cooperative scheduler

The engine does not preempt. It runs `advance()` synchronously until all paths are suspended at Invoke leaves, then yields the accumulated dispatches. External completions arrive via `on_task_completed()`, which triggers `complete()` or `error()`, which may call `advance()` again, producing more dispatches.

This is a **cooperative multitasking** scheduler. Each Invoke is a yield point. The engine runs until it yields, processes one external event, runs until it yields again, and so on. There is no preemption, no timeslicing, no thread scheduling.

The pattern is identical to:
- **Node.js event loop**: run synchronous code until it's done, then process the next event from the queue.
- **Erlang/BEAM scheduler**: run a process until it yields (at a receive or after N reductions), then schedule the next process.
- **Async/await runtimes** (Tokio, Go scheduler): run a future/goroutine until it hits an await point, then park it and run another.

The difference: those systems multiplex many tasks on shared threads. Barnum's engine runs a single workflow — all concurrency is *within* the workflow (Parallel/ForEach), not *between* workflows. Multiple workflows would each have their own Engine instance.

## No closures, no higher-order functions

Barnum is **first-order**. Actions are data (an enum), not functions. You can't pass an action as input to a handler, and handlers can't return actions. There's no lambda, no closure, no partial application.

This is a deliberate constraint. First-order programs are fully inspectable — the flat table is a complete, static description of all possible control flow. You can analyze, optimize, and visualize the entire workflow without executing it. Higher-order functions would make the control flow dynamic and opaque.

The closest analogy is **SQL** — a declarative, first-order language where the query plan is fully determined before execution. Or **shader languages** (GLSL, HLSL) — first-order, fully inspectable, compiled to a flat instruction stream for hardware execution.

## Handler interning is a constant pool

`flatten()` deduplicates identical handlers and assigns each unique handler a `HandlerId`. The engine's handler table is a **constant pool** — a side table of deduplicated constants referenced by index from the instruction stream.

JVM class files have a constant pool for strings, class references, and method descriptors. Lua bytecodes reference a constant table. Barnum's handler table serves the same purpose: avoid duplicating handler metadata (module path, function name, schemas) across every Invoke that uses the same handler.

## Features Barnum lacks

### Variables and assignment

Barnum has no variables. Data flows through the pipeline — each action receives an input value and produces an output value. There's no `let x = ...`, no mutable state, no assignment.

**Does it matter?** Partially. The lack of variables means you can't name intermediate results and reference them later. In `pipe(A, B, C)`, B receives A's output and C receives B's output, but C cannot access A's output — it's gone. The workaround is `parallel(identity(), A)` followed by `merge()` to carry the original input alongside A's output. This is verbose but works.

The deeper question: should Barnum have a `let` binding? Something like `let("x", A, B_that_references_x)`. This would compile to the parallel+identity+merge pattern, but with a cleaner surface syntax. The engine wouldn't change — `let` would desugar during compilation, not require a new frame type.

The functional programming analogy: Barnum pipelines are point-free (tacit) composition. Variables would enable pointed (named) style. Point-free is fine for short pipelines but becomes unreadable for complex data plumbing. Most real-world functional code uses `let` bindings (Haskell's `do` notation, OCaml's `let ... in`, Rust's `let`).

**Verdict:** Matters for ergonomics. Doesn't matter for expressiveness (the workaround exists). A `let` combinator as syntactic sugar over parallel+identity+merge would be a good addition to the TS surface DSL without any engine changes.

### Closures and higher-order functions

Barnum is first-order. Actions are static data, not runtime values. A handler cannot receive an action as input, construct an action dynamically, or return an action. The flat table is fixed at compile time.

**Does it matter?** This is the most significant limitation. Higher-order functions enable abstraction — `map(f, list)` works for any `f`. In Barnum, `ForEach` is a fixed combinator that applies *one specific action* to each element. You can't write a generic "apply this action to that data" combinator in userland.

But the question is whether workflow authors *need* this. Workflows are typically concrete: "call this API, transform the result, branch on status." They rarely need the kind of abstraction that higher-order functions provide. The TS surface DSL provides the abstraction layer — helper functions that generate AST fragments:

```typescript
// This is a function in TypeScript, not in Barnum.
// It produces a static AST fragment.
function retryWithBackoff(action: TypedAction, maxRetries: number): TypedAction {
  return loop(pipe(
    attempt(action),
    branch({ Ok: done(), Err: checkRetryCount(maxRetries) }),
  ));
}
```

The abstraction lives in the metaprogram (TS), not in the object program (Barnum). This is the same split as C++ templates (compile-time abstraction) vs runtime polymorphism, or Lisp macros vs runtime functions.

**Verdict:** Doesn't matter, because the TS metaprogram provides abstraction. The Barnum language itself is intentionally first-order — full inspectability of the flat table is worth more than runtime abstraction.

### Recursion and function calls

Barnum has `Step` (goto) but no function calls. A step reference is a jump, not a call — there's no return address, no stack frame, no parameter passing. You can jump to a named step, but you can't "call" it and return to where you were.

**Does it matter?** Steps are used for mutual recursion (A jumps to B, B jumps to A) and for shared workflow fragments (multiple paths jump to the same cleanup step). Both work fine with goto semantics — the "return" is implicit in the workflow structure (the step's continuation is whatever comes after it in the chain).

True function calls would require a call stack: push a return address, jump to the function, execute, pop the return address, jump back. This would let you reuse the same step body from multiple call sites, each returning to a different point. Currently, if two different chains both need to call the same step and then do different things afterward, they each need their own copy of the chain-with-step — the step doesn't "return" to the caller.

The workaround: duplicate the step reference in each call site. `pipe(step("Validate"), A)` and `pipe(step("Validate"), B)` both jump to Validate, but Validate's continuation is A or B respectively because Chain's `rest` is different in each case. The step body runs the same code; the Chain frame provides the "return address." So Chain already provides function-call semantics for the common case.

**Verdict:** Doesn't matter. Chain + Step gives you effective function calls. The lack of a formal call/return mechanism is a non-issue because the Chain trampoline serves as the return mechanism.

### Mutable shared state

Barnum has no shared state between concurrent branches. Parallel children each receive a clone of the input; they cannot communicate, share mutable state, or observe each other's progress.

**Does it matter?** This is a feature, not a limitation. Shared mutable state in concurrent systems causes races, deadlocks, and nondeterminism. Barnum's fork-join model guarantees deterministic ordering (results are collected in array order, not completion order) and eliminates data races by construction.

If a workflow needs to coordinate between branches, the answer is: don't use Parallel. Use Chain to sequence the dependent parts. Or use a handler that writes to an external database and another that reads from it — the coordination happens outside Barnum, mediated by external state.

**Verdict:** Doesn't matter. Deliberate constraint for correctness.

### Dynamic dispatch / polymorphism

Branch provides static dispatch: the cases are fixed at compile time, and the engine looks up the matching case by string key. There's no dynamic dispatch — you can't choose which action to run based on a runtime-computed action reference.

**Does it matter?** Workflow branching is almost always on a known, finite set of variants (success/failure, request type, user role). Open-ended dispatch (where the set of cases isn't known at compile time) would undermine the static inspectability of the flat table. If the target ActionId is computed at runtime, you can't analyze the workflow's possible paths without executing it.

**Verdict:** Doesn't matter. Static dispatch covers all practical workflow branching patterns.

### String manipulation, arithmetic, data transformation

Barnum has no built-in operations on values. It can route, fork, join, and loop — but it can't add two numbers, concatenate strings, or access object fields (without a handler).

**Does it matter?** Yes, for performance. Every trivial operation (extract a field, wrap in an object, compare values) currently requires a handler invocation — a round trip through the runtime's dispatch mechanism, possibly including serialization. The DEFERRED_FEATURES.md "Builtin Handler Kind" addresses this: Rust-native operations (identity, tag, merge, flatten, extractField) that execute inline without FFI.

Without builtins, Barnum is like a CPU with no ALU — it can branch and jump but can't compute. Builtins are the ALU.

**Verdict:** Matters. Builtins are on the roadmap (DEFERRED_FEATURES.md). The engine doesn't change — builtins are a new `HandlerKind` variant that the runtime executes inline instead of dispatching externally.

### Conditional expressions (if/else without tagged unions)

Branch requires the input to be a tagged union (`{ kind: "...", ... }`). There's no `if (condition)` that evaluates a boolean expression. To branch on a boolean, you'd need a handler that converts `true`/`false` to `{ kind: "True" }` / `{ kind: "False" }`, then Branch on that.

**Does it matter?** It's clunky for simple boolean conditions. But tagged unions are strictly more general than booleans — they carry data per variant, enable exhaustiveness checking, and compose with the type system. The TS surface DSL could provide an `ifElse(condition, thenAction, elseAction)` combinator that desugars to `pipe(condition, branch({ True: thenAction, False: elseAction }))`.

**Verdict:** Minor ergonomics issue. Solvable with a TS combinator, no engine changes.

### Context, effects, and the variable problem

DEFERRED_FEATURES.md describes two related concepts: read-only context (API keys, tenant config) and write-only effects (logging, metrics). These map to well-known PL concepts:

- **Context is Reader** — a value available to all handlers without flowing through the data pipeline. In Haskell, `ReaderT env`. In React, `useContext`. In dynamic languages, thread-local variables.
- **Effects are Writer** — capabilities provided by the host that handlers can invoke. In algebraic effect systems (Koka, Eff, OCaml 5), these are effect handlers. In Haskell, `WriterT` or `IO` actions.

The interesting question: **are variables just context?**

Consider what `let x = A` means: "run A, bind the result to `x`, make `x` available to everything downstream." This is *exactly* dynamic scoping — `x` is available to all descendants in the frame tree without being passed through the data pipeline. A `let` binding is a `Provide` frame that pushes a named value into scope; downstream handlers `Consume` that value by name.

If Barnum had a general `Provide`/`Consume` mechanism (already speculated in DEFERRED_FEATURES.md under "Attempt as Dynamic-Scope Context"):

```typescript
// "let" as a combinator:
provide("userId", extractField("userId"), pipe(
  // ... everything in here can consume("userId")
  handler1(),
  parallel(
    handler2(),  // can access userId without it being in the pipeline value
    handler3(),
  ),
))
```

This is more powerful than the parallel+identity+merge workaround because:
1. **It crosses fan-out boundaries.** All Parallel children can access the variable. With the merge workaround, you'd need to thread the variable through every branch.
2. **It crosses step boundaries.** A variable provided in one step is available in steps it calls. No need to pass it through the pipeline.
3. **It's named, not positional.** You reference `"userId"` by name, not by destructuring a tuple.

The implementation in the engine: `Provide { name, child }` is a frame that stores a named value. `Consume { name }` walks up the frame tree (like error propagation walks up looking for Attempt) and reads the nearest value with that name. This is textbook **dynamic scoping** — the value is determined by the runtime call chain, not the lexical structure.

Dynamic scoping is generally considered harmful in programming languages (hard to reason about, action-at-a-distance). But in a workflow engine, the "call chain" *is* the workflow structure, which is statically known. The flat table tells you exactly which Provide frames are ancestors of which Consume frames. So while the mechanism is dynamic, the analysis can be static.

**Attempt is already a special case of this.** `Attempt` provides an error boundary; `error()` propagation consumes it by walking up. A general Provide/Consume mechanism would subsume Attempt, plus handle:
- **Context (read-only config):** `Provide("apiKey", constant(key), workflow)` — all handlers inside can access the key.
- **Variables (intermediate results):** `Provide("userId", extractField("userId"), rest)` — downstream actions can access userId.
- **Retry policies:** `Provide("retryPolicy", constant({max: 3}), body)` — error handlers check for retry policy.
- **Tracing spans:** `Provide("traceSpan", createSpan(), body)` — handlers emit traces with span context.

The engine change is modest: a new `FrameKind::Provide { name, value }` and a `consume(name)` function that walks `parent` pointers upward. The frame tree walk is the same algorithm as error propagation — it's already implemented.

**Verdict:** Variables, context, and effects are all instances of the same mechanism: named dynamic scope over the frame tree. A single Provide/Consume primitive would handle all three. Worth pursuing once the core engine is stable.

### Error typing

Errors are untyped (`String` in the engine, `unknown` in TS). Handlers can fail with any error, and `Attempt` catches everything. There's no way to specify what errors a handler can produce, or to catch only specific error types.

**Does it matter?** For production workflows, typed errors are important. "This handler can fail with `RateLimitError` or `AuthError`" enables per-error-type handling (retry on rate limit, abort on auth failure). Currently this requires the handler to catch its own errors and return a tagged result — the error boundary can't distinguish error types.

**Verdict:** Matters for production use. Listed in DEFERRED_FEATURES.md as "Handler Error Type."

## What Barnum is not

- **Not Turing-complete on its own.** Without handlers, the engine can only loop forever or terminate immediately. Handlers provide all computation. The engine is a scheduler, not a computer.
- **Not a process calculus.** No communication between concurrent branches (Parallel children can't send messages to each other). Concurrency is fork-join only.
- **Not a dataflow language.** Data flows sequentially through chains and fans out through Parallel, but there are no feedback loops or streaming — each value is produced once and consumed once.
- **Not a monad stack.** Though the patterns are monadic (Chain is bind over an implicit Result, Attempt is catch, Loop is a fixed-point — see "Speculation: Result as the fundamental type" above), there's no monad transformer composition. Each combinator is a hardcoded interpreter case, not a composable effect handler. The engine implements the Result monad's bind as two separate code paths (complete + error) rather than one function with a conditional.
