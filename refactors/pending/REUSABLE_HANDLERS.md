# Reusable TypeScript Handlers

## Motivation

TypeScript handlers today are coupled to the workflow they live in. The handler's `handle()` method returns `[{ kind: "NextStep", value: ... }]`, where `kind` is a step name from the config. A handler must know the graph topology of the workflow that calls it.

Consider a reusable `ts-check` handler that runs `tsc --noEmit` and returns files with type errors. In one workflow, the next step is "Fix". In another, the next step is "Report". The handler can't be shared between them because it hardcodes the step name in its return value.

The goal: TypeScript handlers return plain values. The config controls routing.

## Current state

A TypeScript handler (`crates/barnum_cli/demos/typescript-handler/handler.ts`):

```ts
export default createHandler({
  stepConfigValidator,
  getStepValueValidator(_stepConfig) { return stepValueValidator; },
  async handle({ value }) {
    return [{ kind: "Done", value: { greeting: `Hello, ${value.name}!` } }];
    //            ^^^^^^ hardcoded step name — handler must know the graph
  },
});
```

Handlers are opaque `Handler<C, V>` objects created via `createHandler()`, which captures the file path via stack trace. Configs reference handlers as imported values — no raw path strings in the user-facing API. `fromConfig` resolves Handler objects into serialized TypeScript actions (with file path, stepConfig, valueSchema) synchronously before Zod validation. See `refactors/past/OPAQUE_HANDLER.md` for the full design.

The bridge script (`libs/barnum/actions/run-handler.ts`) passes the result through:

```ts
const results = await handler.handle(envelope);
process.stdout.write(JSON.stringify(results));
```

The Rust runner reads stdout as `Vec<Task>` and validates each task's step against the step's `next` list.

The coupling chain: handler returns step names -> bridge passes them through -> Rust validates against `next`. The handler must know the graph.

## Action kind as an AST

Actions are the AST of an automatically-asynchronous programming language. Every workflow is a program; every action is a node in that program's syntax tree.

The AST has two separate levels:

**Executors** define *how* computation runs. An executor is a runtime mechanism: spawn a shell, invoke a TypeScript handler, etc. Executors are leaf-level — they do actual work.

**Workflow primitives** define *how computations compose*. A workflow primitive is a structural operator: run things in sequence, in parallel, with error boundaries, etc. Workflow primitives are the control flow of the language.

These two levels are orthogonal.

### Executors

Two fundamental kinds: inline code (a string, interpreted) and handlers (a file, loaded and invoked).

```ts
// Internal serialized form (what goes to Rust)
type Executor =
  | { kind: "Inline"; executor: InlineExecutor }
  | { kind: "Handler"; executor: HandlerExecutor }

type InlineExecutor =
  | { kind: "Bash"; source: string }
  | { kind: "TypeScript"; source: string }

type HandlerExecutor =
  | { kind: "Bash"; path: string }
  | { kind: "TypeScript"; path: string;
      exportedAs?: string; stepConfig?: unknown; valueSchema?: unknown }
```

The Handler executor is never written directly by users — it's produced internally by `fromConfig` when resolving `Handler` objects. Users write `createHandler()` and import handlers into configs. See `refactors/past/OPAQUE_HANDLER.md`.

Currently only inline Bash and TypeScript handlers are implemented. Inline TypeScript (serialized via `fn.toString()`) and Bash handlers (external `.sh` files) are future additions — see "Speculation: the full 2×2" below.

Executors never appear at the action level. They are always wrapped in a workflow primitive (Unit).

### Workflow primitives

| Primitive | Semantics |
|-----------|-----------|
| **Unit** | Execute a single executor. The leaf — actually does work. |
| **Sequence** | Function composition. `Sequence([A, B, C])` = `C(B(A(input)))`. Syntactic sugar for chained maps — expressible via step routing, but avoids polluting the graph with intermediate steps. |
| **All** | Run actions in parallel. Wait for all to complete. Collect results. |
| **Try** | Run an action. On success: `{ kind: "Ok", value }`. On failure: `{ kind: "Err", error }`. |
| **Step** | Dispatch a task to a named step. Wait for that task and all its recursive descendants. Return the result. |

### What's NOT a primitive

**Race / first-to-complete.** Timeouts are per-step config, not a composition operator. Non-deterministic choice adds cancellation semantics for a pattern that's rare in workflow land.

**Map / fan-out.** Derivable. A handler returns `[{kind: "Process", value: item1}, {kind: "Process", value: item2}]` and the engine fans out. Dynamic parallelism is already in the data model.

**Conditional / match.** Branching is expressed by handler return values + the step graph. A handler returns `{kind: "Fix", ...}` or `{kind: "Done", ...}` and the engine routes. The handler already decides.

**Compensation / saga.** Expressible as `Sequence(Try(Step("charge")), route-based-on-result)`. Not a primitive.

**Wait / signal.** External event ingestion (webhooks, human approval, timers). Possibly orthogonal to action composition — but the concurrency slot problem (see "Speculation: Suspend" below) suggests this isn't fully settled.

### The complete types

```rust
/// How computation runs. Always wrapped in a workflow primitive.
#[serde(tag = "kind")]
pub enum Executor {
    Inline(InlineExecutor),
    Handler(HandlerExecutor),
}

#[serde(tag = "kind")]
pub enum InlineExecutor {
    Bash { source: String },
    TypeScript { source: String },
}

#[serde(tag = "kind")]
pub enum HandlerExecutor {
    Bash { path: String },
    TypeScript {
        path: String,
        exported_as: Option<String>,
        step_config: Option<Value>,
        value_schema: Option<Value>,
    },
}

/// How computations compose. The AST of the workflow language.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum Action {
    Unit { executor: Executor },
    Sequence(SequenceAction),
    All(AllAction),
    Try(TryAction),
    Step(StepAction),
}
```

```ts
// Internal serialized form (mirrors Rust, produced by fromConfig resolution)
type Executor =
  | { kind: "Inline"; executor: InlineExecutor }
  | { kind: "Handler"; executor: HandlerExecutor }

type InlineExecutor =
  | { kind: "Bash"; source: string }
  | { kind: "TypeScript"; source: string }

type HandlerExecutor =
  | { kind: "Bash"; path: string }
  | { kind: "TypeScript"; path: string;
      exportedAs?: string; stepConfig?: unknown; valueSchema?: unknown }

// User-facing executor input (what users write in configs)
type ExecutorInput =
  | { kind: "Inline"; executor: { kind: "Bash"; source: string } }
  | Handler<any, any>

type Action =
  | { kind: "Unit"; executor: ExecutorInput }
  | { kind: "Sequence"; actions: Action[] }
  | { kind: "All"; actions: Action[] }
  | { kind: "Try"; action: Action }
  | { kind: "Step"; step: string }
```

## Patterns from primitives

The point of good primitives: complex patterns fall out as compositions.

### Pipeline (decouple handler from routing)

The motivating use case. A TypeScript handler returns plain data; a Bash action converts it to `[{kind, value}]` tasks.

```ts
import tsCheck from "@barnum/ts-check";

{
  name: "Check",
  action: {
    kind: "Sequence",
    actions: [
      { kind: "Unit", executor: tsCheck },
      { kind: "Unit", executor: { kind: "Inline", executor: { kind: "Bash", source: `jq 'if .failedFiles | length > 0
        then [{kind: "Fix", value: .}]
        else [{kind: "Done", value: {}}]
        end'` } } },
    ],
  },
  next: ["Fix", "Done"],
}
```

### Cleanup after subtree (replacing `finally`)

The `finally` hook is being removed. It's a special-case mechanism that becomes unnecessary once the primitives exist. Sequence + All + Step express cleanup as a composition:

```ts
import processHandler from "./process.js";
import cleanupHandler from "./cleanup.js";

{
  name: "Process",
  action: {
    kind: "Sequence",
    actions: [
      { kind: "Unit", executor: processHandler },
      { kind: "All", actions: [
        { kind: "Step", step: "SubtaskA" },
        { kind: "Step", step: "SubtaskB" },
      ]},
      { kind: "Unit", executor: cleanupHandler },
    ],
  },
  next: ["SubtaskA", "SubtaskB"],
}
```

This is strictly more powerful: cleanup gets the subtree results, you can have multiple wait points, and you can compose with Try.

### Error recovery

Step fails → catch the error → route to recovery instead of retrying.

```ts
{
  name: "SafeProcess",
  action: {
    kind: "Sequence",
    actions: [
      { kind: "Try", action: { kind: "Step", step: "RiskyWork" } },
      { kind: "Unit", executor: { kind: "Inline", executor: { kind: "Bash", source: `jq 'if .kind == "Ok"
        then [{kind: "Continue", value: .value}]
        else [{kind: "Recover", value: .error}]
        end'` } } },
    ],
  },
  next: ["Continue", "Recover"],
}
```

### Terminal step (no-op)

Currently requires a Bash hack (`echo '[]'`). This should not need a subprocess. Options:

1. **Optional executor on Unit.** `{ kind: "Unit" }` with no executor. But what does it do? As a standalone step action, it should produce `[]` (consume the task, spawn nothing). In a Sequence, it should pass input through (identity). Context-dependent behavior from the same AST node is a design smell — the meaning of the node changes based on where it appears.

2. **Noop executor.** `{ kind: "Unit", executor: { kind: "Noop" } }`. Always produces `[]`, regardless of context. Explicit, no ambiguity. Slightly verbose, but terminal steps are a small fraction of configs.

3. **Dedicated Terminal action.** `{ kind: "Terminal" }`. A workflow primitive, not an executor — it means "this step consumes tasks and produces nothing." Cleanly separates "do nothing and return empty" (Terminal) from "do nothing and pass through" (which would be a separate Identity primitive or optional executor on Unit).

Option 2 is the safest — unambiguous semantics, no context-dependence. Option 3 is cleaner if we also need an identity/passthrough (which Sequence would want). Option 1 is tempting but the dual behavior is a trap. Open question.

## Handler definition types

Currently, `HandlerDefinition` is a single interface. All handlers return `FollowUpTask[]` and are created via `createHandler()`:

```ts
// Current
interface HandlerDefinition<C = unknown, V = unknown> {
  stepConfigValidator: z.ZodType<C>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V>;
  handle: (context: HandlerContext<C, V>) => Promise<FollowUpTask[]>;
}

export default createHandler({
  stepConfigValidator,
  getStepValueValidator(_stepConfig) { return stepValueValidator; },
  async handle({ value }) { return [{ kind: "Done", value: {} }]; },
});
```

For Sequence support, `HandlerDefinition` should become a discriminated union on `kind`:

```ts
type HandlerDefinition<C = unknown, V = unknown> =
  | RoutingHandler<C, V>
  | ValueHandler<C, V>;

interface RoutingHandler<C = unknown, V = unknown> {
  kind: "Routing";
  stepConfigValidator: z.ZodType<C>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V>;
  handle: (context: HandlerContext<C, V>) => Promise<FollowUpTask[]>;
}

interface ValueHandler<C = unknown, V = unknown> {
  kind: "Value";
  stepConfigValidator: z.ZodType<C>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V>;
  handle: (context: HandlerContext<C, V>) => Promise<unknown>;
}
```

`RoutingHandler`: sole action or last action in a sequence — returns `[{kind, value}]` tasks.
`ValueHandler`: non-final position in a sequence — returns plain data piped to the next action.

Both are created via `createHandler()` which wraps them in an opaque `Handler<C, V>` object.

## What to ship when

### Ship now (done)

TypeScript handler support with validation, synchronous `run()` API, troupe decoupled, opaque `Handler<C, V>` type via `createHandler()`. Already shipped.

### Ship soon (Rust, before JS rewrite)

**Unit** and **Sequence**. Both are trivial in the Rust subprocess model:

- **Unit**: dispatch to the wrapped executor. One level of indirection, zero new complexity.
- **Sequence**: spawn each child action sequentially, capture intermediate stdout, feed as stdin to the next. Only the final action's stdout goes through response parsing.

Requires:
- Split current `ActionKind` into `Executor` + `Action` enums in `config.rs`
- Add `Sequence` variant to `Action`
- Piping logic in the runner
- Schema regeneration
- Update existing configs: `{ kind: "Bash", script: "..." }` → `{ kind: "Unit", executor: { kind: "Inline", executor: { kind: "Bash", source: "..." } } }` and `handler` → `{ kind: "Unit", executor: handler }`

### Ship with JS rewrite

**All**, **Try**, **Step**. These require the action to *suspend* — wait for subtrees to complete before continuing. In the Rust subprocess model, actions are fire-and-forget. In a JS runtime, these are just `await` calls.

- **All**: `await Promise.all(actions.map(run))`
- **Try**: `try { await run(action) } catch (e) { ... }`
- **Step**: `await dispatch(stepName, value)` — dispatch a task to the named step, wait for its entire subtree

`finally` is removed when All + Step ship — it's just Sequence + All + Step.

## Changes required for Unit + Sequence

### Rust side (`crates/barnum_config`)

**config.rs** — Split `ActionKind` into two enums:

```rust
#[serde(tag = "kind")]
pub enum Executor {
    Inline(InlineExecutor),
    Handler(HandlerExecutor),
}

#[serde(tag = "kind")]
pub enum InlineExecutor {
    Bash { source: String },
    TypeScript { source: String },
}

#[serde(tag = "kind")]
pub enum HandlerExecutor {
    Bash { path: String },
    TypeScript { path: String, exported_as: Option<String>,
        step_config: Option<Value>, value_schema: Option<Value> },
}

#[serde(tag = "kind")]
pub enum Action {
    Unit { executor: Executor },
    Sequence(SequenceAction),
}

pub struct SequenceAction {
    pub actions: Vec<Action>,
}
```

**config.rs validation** — Sequence must have at least one action. Empty sequence is a config error.

**runner dispatch**:

```
match action:
  Unit { executor } ->
    match executor:
      Inline(inline) -> match inline:
        Bash { source } -> spawn bash, pipe stdin, return stdout
        TypeScript { source } -> eval serialized fn, pipe stdin, return stdout
      Handler(handler) -> match handler:
        Bash { path } -> spawn bash file, pipe stdin, return stdout
        TypeScript { path, .. } -> spawn ts handler, pipe stdin, return stdout
  Sequence(seq) ->
    let data = envelope_stdin
    for action in seq.actions:
      data = dispatch_action(action, data)
    return data
```

For Sequence, intermediate actions get their predecessor's stdout as stdin. All actions in the sequence share the same timeout (the step-level timeout applies to the entire sequence, not individual actions). A failure at any point retries the whole sequence from the beginning.

### TypeScript side (`libs/barnum`)

**run.ts** — `resolveHandlers()` (called by `fromConfig`) unwraps Unit to find Handler objects, walks into Sequence actions to find nested Handlers, and resolves each into a serialized TypeScript executor with file path, stepConfig, and valueSchema.

### Generated schemas

Regenerated. `ActionKind` is replaced by `Executor` + `Action`.

## Future: deriving `next` from the handler's return schema

Long-term, the `next` array on a step shouldn't need to be manually specified. It should be derivable from the handler's return type.

A handler already declares a Zod schema for its input value via `getStepValueValidator()`. If the handler (or the final action in a sequence) also declares a Zod schema for its *return* type, the engine can introspect that schema to discover step references. The rule: any `kind` field anywhere in the return schema is a step reference. The set of possible values for that field (literal strings, enum members) is the set of reachable steps.

### Example

A handler returns one of two task shapes:

```ts
const returnSchema = z.array(z.union([
  z.object({ kind: z.literal("Fix"), value: z.object({ files: z.array(z.string()) }) }),
  z.object({ kind: z.literal("Done"), value: z.object({}) }),
]));
```

The engine walks `returnSchema`, finds `kind` fields with values `"Fix"` and `"Done"`, and derives `next: ["Fix", "Done"]`. If the config manually specifies `next`, it's validated against the derived set. If the config omits `next`, the derived set is used.

### When to implement

JS rewrite. The Zod tree walking is straightforward, but it needs to happen in JS (where the Zod objects live). In a pure JS engine, the return schema lives alongside the step graph and validation is trivial.

## Open questions

1. **Nested sequences.** Allow `Sequence` inside `Sequence`? Yes — flatten at runtime. No reason to forbid it, keeps the type system simple.

2. ~~**Sequence + finally.**~~ Removed. `finally` is being deleted — see "Cleanup after subtree" section.

3. **Sequence + retries.** If any action in a sequence fails, does the entire sequence retry from the beginning? Yes — the sequence is one atomic action from the retry system's perspective.

4. **Sequence + timeout.** The step-level timeout applies to the entire sequence execution, not individual actions. The sequence is one unit of work.

5. **Terminal steps.** How should no-op terminal steps work without a Bash `echo '[]'` hack? Options: optional executor on Unit (context-dependent semantics — design smell), Noop executor (explicit), or dedicated Terminal primitive. See "Terminal step" section above.

## Speculation: the full 2×2

The Executor type has two axes: `{Inline, Handler} × {Bash, TypeScript}`. Today only two cells are implemented:

| | Inline | Handler |
|---|---|---|
| **Bash** | `{ kind: "Inline", executor: { kind: "Bash", source } }` | `{ kind: "Handler", executor: { kind: "Bash", path } }` |
| **TypeScript** | `{ kind: "Inline", executor: { kind: "TypeScript", source } }` | `{ kind: "Handler", executor: { kind: "TypeScript", path, stepConfig, ... } }` (via `createHandler`) |

**Implemented now**: Inline Bash, TypeScript Handler.

**Future**: Inline TypeScript (serialized via `fn.toString()` — closures that capture locals will break, use a Handler instead). Bash Handler (external `.sh` file — useful for non-trivial shell scripts that don't belong inline in a config).

New languages (Python, etc.) are additional variants on `InlineExecutor` and `HandlerExecutor`.

## Speculation: Suspend

### The problem

A step waits for user approval. With `max_concurrency=3`, that task holds a slot indefinitely. Effective concurrency drops to 2. If three tasks all wait for approval, the entire workflow stalls — zero throughput while every slot is blocked on a human.

This is the concurrency slot problem. The task isn't doing work, but the engine doesn't know that. It looks the same as a long-running computation.

### Is Suspend a primitive?

The irreducibility test: can Suspend be expressed as a combination of Unit, Sequence, All, Try, and Step?

No. None of the existing primitives express "release your concurrency slot and wait for an external signal." Unit runs to completion. Step dispatches and waits, but it's still waiting on *computation*, not on an *external event*. Try observes errors. Sequence and All compose things that are already running.

Suspend is a statement about the task's *lifecycle*, not about how computations *compose*. That's the tension. The action AST describes composition. Suspend describes resource management.

### Where it could live

**Option 1: Action primitive.**

```ts
type Action =
  | { kind: "Unit"; executor: ExecutorInput }
  | { kind: "Sequence"; actions: Action[] }
  | { kind: "Suspend" }   // park here, resume externally
  | ...
```

In a Sequence: `Sequence([doWork, Suspend, continueAfterResume])`. The engine checkpoints the sequence state at the Suspend point, frees the concurrency slot, and resumes from the next action when signaled.

Pro: composable. You can put Suspend anywhere in a Sequence. The AST explicitly marks where suspension happens.

Con: the Sequence is now stateful. The engine must persist "we're on action 2 of 4, here's the intermediate data" to resume later. Today, sequences are ephemeral — run start-to-finish in one shot. Suspend breaks that model.

**Option 2: Handler return value.**

The handler decides when to suspend. Instead of returning follow-up tasks, it returns a suspend signal:

```ts
// Handler result becomes a union
type HandlerResult =
  | FollowUpTask[]                              // done, here are next tasks
  | { kind: "Suspend"; resumeValue?: unknown }  // park me, I'll be back

// Handler code
async handle({ value }) {
  if (needsApproval(value)) {
    return { kind: "Suspend" };
  }
  return [{ kind: "Next", value: result }];
}
```

The engine sees the Suspend return, writes the task to the state log as suspended, frees the slot. An external signal (`barnum resume <task-id> --value '{approved: true}'`) re-queues the task. On resume, the handler's context includes the resume signal:

```ts
interface HandlerContext<C, V> {
  stepConfig: C;
  value: V;
  config: unknown;
  stepName: string;
  resumeValue?: unknown;  // present only on resume, contains the value from `barnum resume`
}
```

The handler checks `resumeValue` to distinguish first run from resume:

```ts
async handle({ value, resumeValue }) {
  if (resumeValue) {
    // Resumed — approval decision is in resumeValue
    return resumeValue.approved
      ? [{ kind: "Next", value: result }]
      : [{ kind: "Rejected", value: {} }];
  }
  // First run — need approval
  return { kind: "Suspend" };
}
```

Pro: no change to the action AST. The handler has context to decide *when* to suspend. Simpler engine changes — suspended is just another task state alongside pending/running/completed. The `resumeValue` field makes first-run vs resume unambiguous.

Con: the handler runs twice. The original `value` is preserved across suspend/resume, but the handler must handle both code paths. If the handler has side effects on first run (e.g., sent an email requesting approval), it must not re-trigger them on resume.

**Option 3: Task state, not action primitive.**

Suspend isn't in the AST at all. It's a task lifecycle state managed by the engine:

- Any task can be suspended externally (`barnum suspend <task-id>`)
- Suspended tasks don't count against `max_concurrency`
- `barnum resume <task-id>` re-queues them
- The task reruns from scratch (same step, same value, or updated value)

Pro: zero changes to handlers or the action AST. Pure runtime/scheduling concern.

Con: coarse-grained. You can't suspend mid-Sequence. The whole task reruns on resume. No way for the handler to say "I need to wait" — it's always an external decision.

### The durable execution question

Options 1 and 2 both imply **durable execution state**. If a task suspends and the engine restarts, the suspended task must survive. This is already handled by the state log — suspended tasks would be entries in the log, replayed on resume.

But Option 1 (Suspend as AST primitive in a Sequence) requires persisting *mid-sequence state*: which action we're on, what intermediate data was produced. This is significantly more complex than persisting task-level state. It's the difference between "this task is parked" (simple) and "this task is paused at instruction 3 of 7 with this stack" (durable execution runtime, a la Temporal).

### The resume signal

All options punt on the hardest question: what triggers resume? Candidates:

- **CLI command**: `barnum resume <task-id> --value '...'`. Manual, simple, works today.
- **Webhook**: engine exposes an HTTP endpoint. External system calls it. Requires the engine to run an HTTP server.
- **Named event**: handler suspends with `{ event: "approval-granted" }`. Something publishes that event. Requires a pub/sub system.
- **Timer**: resume after N seconds. Engine manages a timer queue.

For now, CLI command is sufficient. The others are additive.

### Where this lands

Probably **Option 2** (handler return value) is the right first step. It's the smallest change — suspended is a task state, handlers opt in, the state log already supports it. No AST changes, no mid-sequence checkpointing.

Option 1 (AST primitive) becomes interesting once we have the JS engine and durable Sequence execution. At that point, Suspend in a Sequence is just `await externalSignal()` — the JS runtime handles the checkpointing naturally.

Don't act on this yet. But the concurrency slot problem is real, and "just increase max_concurrency" is not the answer.
