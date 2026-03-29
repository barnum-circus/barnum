# Algebraic Effects: Implementation Roadmap

This document describes the **final state** after all phases are complete. The "What we have now" section is the starting point. "Where we're going" is the end state. The phases section describes the intermediate milestones to get there — see each phase's own doc for intermediate details.

## What we have now

### The tree AST (TypeScript)

7 action node types:

| Node | Role |
|---|---|
| `Invoke` | Call an external handler (TypeScript function) or a builtin |
| `Chain` | Sequential composition (binary: first, rest) |
| `Parallel` | Concurrent fork-join (n-ary: actions[]) |
| `Branch` | Dispatch on tagged union (cases map) |
| `ForEach` | Map action over array |
| `Loop` | Repeat body until Break signal |
| `Step` | Jump to a named step (mutual recursion) |

10 builtin handler kinds: Constant, Identity, Drop, Tag, Merge, Flatten, ExtractField, ExtractIndex, Pick, CollectSome.

### The TS surface layer

`pipe`, `augment`, `tap`, `pick`, `merge`, `option.map`, `withResource`, `forEach`, `loop`, `branch`, `parallel`, `recur`, `done`, `range` are all TypeScript functions that produce trees of the 7 node types. This sugar layer is well-established and works.

The callback pattern used by `declare` (proposed), `loop` (body callback not yet used), and `registerSteps` (batch registration) is **Higher-Order Abstract Syntax (HOAS)**. TypeScript's lexical scoping handles name resolution. The builder generates globally unique IDs (gensym) and constructs opaque AST references that the callback receives. The Rust engine sees a flat, collision-free graph with no symbol tables.

### The Rust side

The Rust flattener lowers the tree AST into a flat action table. The Rust scheduler executes the flat table using a slab-allocated frame tree linked by parent pointers. Frames communicate via `advance` (push work down) and `deliver` (push results up).

### What's missing

No mechanism for a deeply nested node to communicate with a lexical ancestor. Every feature that needs scope — variables, error handling, timeouts, resource cleanup, early return — requires a new AST node type and a new frame kind in the scheduler. This doesn't scale.

## Where we're going

Replace the ad-hoc approach with a single general mechanism: algebraic effects via Handle/Perform.

### The target AST

Replace `Loop` and `Step` with `Handle` and `Perform`. The final node set:

| Node | Role | Status |
|---|---|---|
| `Invoke` | External computation | Exists |
| `Chain` | Sequential composition | Exists |
| `Parallel` | Concurrent fork-join | Exists |
| `Branch` | Coproduct routing | Exists |
| `ForEach` | Functorial map | Exists |
| `Handle` | Install scoped effect handler | **New** |
| `Perform` | Emit effect, suspend execution | **New** |

7 nodes total. `Loop` and `Step` become TS sugar that compiles to Handle/Perform. Every scope-based feature (declare, tryCatch, withTimeout, race, RAII, durable suspension) is TS sugar over these same two primitives.

There is no `Resume` AST node. Resumption (and discarding, and body re-entry) are not AST-level concepts. They are the Handle frame's interpretation of the handler DAG's tagged output. Handler DAGs produce `{ kind: "Resume"|"Discard"|"RestartBody", value }`, and the Handle frame acts accordingly. This keeps cont_id tokens internal to the scheduler — handler DAGs never see them.

### The architectural insight

Two layers, each doing what it's good at:

**TypeScript (HOAS)**: Provides the user-facing API. Callbacks receive opaque AST references (VarRefs, restart/exit jumps, step references). TypeScript's lexical scoping prevents collisions and enforces scope. The builder gensyms unique IDs. TypeScript's type system checks that inputs and outputs match at every connection point. All sugar expansion happens here.

**Rust (Effect substrate)**: Provides the structural routing mechanism. The scheduler knows nothing about what effects mean. It knows: when a Perform fires, walk parent pointers to find a matching Handle, sever the link, dispatch the handler DAG with `{ payload }`. When the handler DAG completes, read its tagged output (`Resume`, `Discard`, or `RestartBody`) and act accordingly. When a Handle frame exits, clean up orphaned continuations. That's it.

The Rust engine is a pure structural router. It understands three universal continuation operations (Resume, Discard, RestartBody) but knows nothing about what effects mean semantically. All semantic meaning (what ReadVar does, what Throw does, what Continue does) lives in the handler DAGs, which are normal workflow graphs written in TypeScript.

### Control Plane / Data Plane boundary

Handlers are opaque, pure computations. They receive input (JSON), return output (JSON). They cannot access the scheduler, cannot see variables, cannot emit effects, cannot pause the engine. This is the strict separation between:

- **Control Plane (AST / Rust engine)**: Manages execution flow. Effects, variable resolution, error routing, timeouts, cancellation. The AST nodes and the scheduler.
- **Data Plane (Handlers / TypeScript workers)**: Opaque computation. Receives data, returns data. No knowledge of the orchestrator's state.

Handlers communicate intent by returning discriminated unions. The AST interprets those unions and translates intent into effects. This is the **Free Monad** pattern: handlers don't execute side-effects on the workflow; they return data structures describing intent. The AST inspects and acts.

```ts
// Handler returns intent:
type ReviewResult =
  | { kind: "Approved" }
  | { kind: "RequiresHuman"; diffUrl: string };

// AST interprets intent (inside a scope that provides suspendEffect):
pipe(
  invoke(automatedReview),
  branch({
    Approved: proceed,
    RequiresHuman: suspendEffect,  // AST emits the effect, not the handler
  }),
)
```

This pattern is already how Barnum works. The `typeCheck -> classifyErrors -> branch` pattern in the demos is exactly intent-returning. The handler classifies errors and returns a tagged union. The AST branches on it. The handler never manipulates the execution graph.

Convenience combinators should wrap common intent patterns. For example, `invokeWithThrow(handler, throwError)` wraps an Invoke + branch on error union + throw:

```ts
function invokeWithThrow<TIn, TOut, TError>(
  handler: Pipeable<TIn, Result<TOut, TError>>,
  throwError: Pipeable<TError, never>,
) {
  return pipe(
    handler,
    branch({
      Ok: pick("value"),
      Err: pipe(pick("error"), throwError),
    }),
  );
}

// Usage — throwError comes from the HOAS callback:
tryCatch(
  (throwError) => invokeWithThrow(riskyHandler, throwError),
  handleError,
)
```

### The effect boundary: in-band vs out-of-band

Not everything should be an effect. The rule:

- **In-band (effects)**: Things that mutate the execution path. Variable lookup, error handling, loop control, suspension, timeout cancellation. These MUST go through Handle/Perform because the scheduler must evaluate them to compute the next graph state.
- **Out-of-band (driver/IPC)**: Things that observe without mutating. Logging, metrics, heartbeats, progress reporting. These MUST NOT go through Handle/Perform. They stream directly to the infrastructure layer.

Logging in particular is out-of-band. The Tokio driver captures stdout/stderr from the worker subprocess. A separate task streams log lines to the observability stack. If the handler is killed by a timeout, logs emitted before the kill are preserved — they were streaming out-of-band, not buffered in a return value.

Do NOT add an `Effect::Log` or `Perform("barnum:log")`. It would route observability through the state machine's evaluation loop, making logs synchronous and coupling them to handler completion. Logging survives handler crashes precisely because it's out-of-band.

### Effect routing: gensym'd opaque IDs

Every Handle-installing combinator (`declare`, `tryCatch`, `loop`, etc.) gensyms a fresh `EffectId` at call time. The Rust engine routes on opaque `u32` IDs — it never interprets effect names.

```ts
// Inside the declare() combinator:
function declare(bindings, bodyCallback) {
  const effectId = generateUniqueId();  // monotonic u32
  const varRefs = createVarRefs(bindings, effectId);
  const body = bodyCallback(varRefs);   // VarRefs are Perform(effectId)
  return Handle({ [effectId]: readVarHandler }, body);
}

// Inside the tryCatch() combinator:
function tryCatch(bodyCallback, recovery) {
  const effectId = generateUniqueId();
  const throwError = Perform(effectId);  // typed as Pipeable<TError, never>
  const body = bodyCallback(throwError);
  return Handle({ [effectId]: recoveryHandler(recovery) }, body);
}
```

```rust
// Rust: opaque ID, no interpretation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct EffectId(pub u32);

pub enum FlatAction {
    Handle { handlers: BTreeMap<EffectId, ActionId>, body: ActionId },
    Perform { effect: EffectId },
}
```

There are no global/module-level effect tokens. Each combinator invocation creates its own EffectId. This avoids both the rigidity of a Rust enum (no recompilation for new effects) and the collision risk of global strings (IDs are unique by construction). A Handle block can only intercept effects whose EffectId it holds — the HOAS callback is the sole distribution mechanism.

The `debugName` string (passed to `generateUniqueId` for diagnostics) is metadata for error messages and telemetry. It never participates in routing.

## The HOAS pattern

What we're already doing — and should do consistently — is Higher-Order Abstract Syntax. The builder provides opaque references via callbacks. The host language (TypeScript) handles scoping.

| Feature | HOAS callback | Opaque reference | Current status |
|---|---|---|---|
| `declare` | `({ x }) => body` | VarRef = `Perform(freshEffectId)` | Proposed |
| `tryCatch` | `(throwError) => body, recovery` | throwError = `Perform(freshEffectId)` | Proposed |
| `loop` | `(recur, done) => body` | recur/done = `Perform(freshEffectId)` | Not yet HOAS |
| `scope` | `(restart, exit) => body` | Jump references | Not yet implemented |
| `registerSteps` | `({ stepRef }) => steps` | Step references | Exists (string-based) |

The pattern is: every combinator that installs a Handle:
1. Gensyms a fresh `EffectId`
2. Creates a `Handle` keyed on that ID
3. Passes `Perform(thatId)` wrappers to the callback as opaque `Pipeable` nodes

TypeScript enforces that references are used within their lexical scope. The builder gensyms the IDs. **There are no global/module-level effect tokens.** The HOAS callback is the sole distribution mechanism for effect tokens. If a utility function needs to throw, it receives the throw token as a parameter — explicit propagation, not ambient authority.

This gives per-Handle precision: in nested tryCatch, `throwOuter` skips the inner handler and targets the outer one directly. In nested loops, `doneOuter` breaks out of both loops. No re-throwing, no labeled-break syntax — just lexical references.

`registerSteps` currently uses user-visible string names (`stepRef("TypeCheck")`). Under HOAS, these would be opaque references from the batch callback. The names become metadata (for logs/errors), not identifiers.

For mutual recursion, HOAS requires batch registration (all nodes pre-allocated before any is defined). `registerSteps` already does this. The HOAS version replaces string keys with opaque references.

## Phases

### Phase 1: Effect Substrate

Build Handle/Perform in the Rust scheduler plus the tagged output interpretation (Resume/Discard/RestartBody). The structural routing mechanism. No semantic effects yet — just the ability for a Perform to suspend, bubble up to a Handle, and be acted on based on the handler DAG's tagged output.

See: `EFFECTS_PHASE_1_SUBSTRATE.md`

### Phase 2: Variable Declarations (ReadVar)

First real effect. Exercises the resume path: effect fires, handler looks up value, resumes immediately. Validates that the mechanism works end to end.

Delivers: `declare` combinator, solves prop drilling.

See: `EFFECTS_PHASE_2_DECLARE.md`

### Phase 3: Error Handling (Throw)

First use of the discard path. When an error fires, the handler drops the continuation and runs a recovery branch. Validates teardown of orphaned frames.

Delivers: `tryCatch` combinator, graceful error recovery.

See: `EFFECTS_PHASE_3_TRYCATCH.md`

### Phase 4: Loop Migration (LoopControl)

Migrate the existing LoopAction to Handle/Perform. Exercises re-entry semantics: on Continue, discard the old continuation and re-enter the body. On Break, discard and exit.

Delivers: Loop works the same but uses the general mechanism. LoopAction removed from the AST. Existing loop tests provide regression coverage.

See: `EFFECTS_PHASE_4_LOOP.md`

### Phase 5: Advanced Patterns (RAII, Race, Timeout)

Three features that stress-test the mechanism in different ways:

- **RAII (Bracket)**: Scope-exit cleanup. When a Handle frame exits, run dispose on acquired resources. Separated from variable binding (Bracket is affine, ReadVar is unrestricted).
- **Race**: First-completion with sibling cancellation. Validates the discard path under Parallel — tearing down a live branch while its sibling has already completed.
- **Timeout**: External timer integration. The external driver fires a cancellation signal after a duration. The scheduler cancels the body and propagates an error.

See: `EFFECTS_PHASE_5_ADVANCED.md` (to be written when Phase 4 is complete)

### Phase 6: Durable Suspension

The workflow serializes its state and goes dormant. An external trigger resumes it later. The continuation is persisted to storage, not just held in memory.

Delivers: pause/resume for human-in-the-loop workflows, webhook-triggered continuation.

Prerequisite: the entire WorkflowState (slab, environment, pending tasks) must be serializable.

See: `EFFECTS_PHASE_6_DURABLE.md` (to be written when Phase 5 is complete)

## Phase dependencies

```
Phase 1 (Substrate)
  ├── Phase 2 (Declare / ReadVar)
  ├── Phase 3 (TryCatch / Throw)
  │     └── Phase 5 (RAII, Race, Timeout) — needs discard path from Phase 3
  └── Phase 4 (Loop migration)
                └── Phase 6 (Durable Suspension) — needs all mechanisms stable
```

Phases 2, 3, and 4 can proceed in parallel after Phase 1. Phase 5 depends on Phase 3 (discard path). Phase 6 depends on everything being stable.

## What changes on each side

### TypeScript changes

- New AST node types: `HandleAction`, `PerformAction` (no ResumeAction — resumption is internal to the Handle frame)
- Each Handle-installing combinator gensyms a fresh `EffectId` and provides `Perform(thatId)` wrappers via HOAS callback
- `declare()` function: HOAS callback provides `VarRef<T>` nodes, compiles to Chain + Handle(freshId) + Perform(freshId)
- `tryCatch()` function: HOAS callback provides `throwError` token, compiles to Handle(freshId) + Perform(freshId)
- `loop()` rewritten: HOAS callback provides `recur`/`done` tokens, compiles to Handle(freshId) + Perform(freshId)
- Standalone `recur()` / `done()` removed — tokens come from the HOAS callback only
- `LoopAction` removed from Action union
- `StepAction` potentially removed (replaced by Handle-based mutual recursion, or kept for backward compat during migration)

### Rust changes

- `EffectId(u32)`: opaque effect routing key
- New flat action types: `FlatHandle`, `FlatPerform`
- New frame kind: `FrameKind::Handle` (one new variant, replaces LoopAction's frame kind)
- `bubble_effect()`: new traversal method on WorkflowState — walks parent pointers to find matching Handle
- Tagged output interpretation: Handle frame reads `{ kind, value }` from handler DAG and performs Resume, Discard, or RestartBody
- Continuation management: sever/reconnect parent links, internal tracking per handler invocation
- Teardown: recursive frame cleanup when continuations are discarded
- `FrameKind::Loop` removed (after Phase 4)

### What doesn't change

- Invoke, Chain, Parallel, Branch, ForEach — completely unchanged in both AST and scheduler
- All existing builtins (Constant, Identity, Drop, Tag, Merge, etc.) — unchanged
- Handler execution (TypeScript subprocess, worker protocol) — unchanged
- The surface API functions (pipe, augment, tap, pick, etc.) — unchanged
- Workflow builder, runner, config serialization — minor updates for new node types
