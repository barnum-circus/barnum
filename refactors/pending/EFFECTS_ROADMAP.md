# Algebraic Effects: Implementation Roadmap

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

Replace `Loop` and `Step` with `Handle`, `Perform`, and `Resume`. The final node set:

| Node | Role | Status |
|---|---|---|
| `Invoke` | External computation | Exists |
| `Chain` | Sequential composition | Exists |
| `Parallel` | Concurrent fork-join | Exists |
| `Branch` | Coproduct routing | Exists |
| `ForEach` | Functorial map | Exists |
| `Handle` | Install scoped effect handler | **New** |
| `Perform` | Emit effect, suspend execution | **New** |
| `Resume` | Inject value into suspended continuation | **New** |

8 nodes total. `Loop` and `Step` become TS sugar that compiles to Handle/Perform/Resume. Every scope-based feature (declare, tryCatch, withTimeout, race, RAII, durable suspension) is TS sugar over these same three primitives.

### The architectural insight

Two layers, each doing what it's good at:

**TypeScript (HOAS)**: Provides the user-facing API. Callbacks receive opaque AST references (VarRefs, restart/exit jumps, step references). TypeScript's lexical scoping prevents collisions and enforces scope. The builder gensyms unique IDs. TypeScript's type system checks that inputs and outputs match at every connection point. All sugar expansion happens here.

**Rust (Effect substrate)**: Provides the structural routing mechanism. The scheduler knows nothing about what effects mean. It knows: when a Perform fires, walk parent pointers to find a matching Handle, sever the link, dispatch the handler DAG with `{ payload, cont_id }`. When Resume fires, reconnect the continuation and deliver the value. When a Handle frame exits with un-resumed continuations, clean them up. That's it.

The Rust engine is a pure structural router. All semantic meaning (what ReadVar does, what Throw does, what Continue does) lives in the handler DAGs, which are normal workflow graphs written in TypeScript.

### Effect routing: strings vs enum

Two options for how Handle identifies which effects it intercepts:

**Enum (recommended for now)**: A closed set of effect types defined in the protocol between TS and Rust. New effects require adding to the enum. Gives exhaustiveness checking in Rust match arms. Appropriate because all planned effects are framework-level, not user-defined.

**String keys (future option)**: Open-ended routing. Any string can be an effect type. The Rust engine matches on strings. No Rust changes for new effects. Appropriate if users ever need custom effects. Migration from enum to strings is straightforward — the scheduler logic doesn't change, only the matching mechanism.

Start with enum. Migrate to strings if we need open-ended effects.

## The HOAS pattern

What we're already doing — and should do consistently — is Higher-Order Abstract Syntax. The builder provides opaque references via callbacks. The host language (TypeScript) handles scoping.

| Feature | HOAS callback | Opaque reference | Current status |
|---|---|---|---|
| `declare` | `({ x }) => body` | VarRef (gensym'd ID) | Proposed |
| `loop` | `(recur, done) => body` | Perform(Continue), Perform(Break) | Not yet HOAS |
| `scope` | `(restart, exit) => body` | Jump references | Not yet implemented |
| `registerSteps` | `({ stepRef }) => steps` | Step references | Exists (string-based) |

The pattern is: `combinator(callback)` where the callback receives opaque AST nodes and returns an AST that uses them. TypeScript enforces that references are used within their lexical scope. The builder gensyms the IDs.

`registerSteps` currently uses user-visible string names (`stepRef("TypeCheck")`). Under HOAS, these would be opaque references from the batch callback. The names become metadata (for logs/errors), not identifiers.

For mutual recursion, HOAS requires batch registration (all nodes pre-allocated before any is defined). `registerSteps` already does this. The HOAS version replaces string keys with opaque references.

## Phases

### Phase 1: Effect Substrate

Build Handle/Perform/Resume in the Rust scheduler. The structural routing mechanism. No semantic effects yet — just the ability for a Perform to suspend, bubble up to a Handle, and be resumed or discarded.

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

- New AST node types: `HandleAction`, `PerformAction`, `ResumeAction`
- `declare()` function: compiles to Chain + Handle(ReadVar) + Perform(ReadVar) + Resume
- `tryCatch()` function: compiles to Handle(Throw) + Perform(Throw), no Resume
- `loop()` rewritten: compiles to Handle(LoopControl) + Perform(Continue/Break)
- `recur()` / `done()` rewritten: compile to Perform(Continue) / Perform(Break)
- `LoopAction` removed from Action union
- `StepAction` potentially removed (replaced by Handle-based mutual recursion, or kept for backward compat during migration)

### Rust changes

- New flat action types: `FlatHandle`, `FlatPerform`, `FlatResume`
- New frame kind: `FrameKind::Handle` (one new variant, replaces LoopAction's frame kind)
- `bubble_effect()`: new traversal method on WorkflowState — walks parent pointers to find matching Handle
- Continuation management: sever/reconnect parent links, cont_id token generation and storage
- Teardown: recursive frame cleanup when continuations are discarded
- `FrameKind::Loop` removed (after Phase 4)

### What doesn't change

- Invoke, Chain, Parallel, Branch, ForEach — completely unchanged in both AST and scheduler
- All existing builtins (Constant, Identity, Drop, Tag, Merge, etc.) — unchanged
- Handler execution (TypeScript subprocess, worker protocol) — unchanged
- The surface API functions (pipe, augment, tap, pick, etc.) — unchanged
- Workflow builder, runner, config serialization — minor updates for new node types
