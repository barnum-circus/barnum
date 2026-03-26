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

These two levels are orthogonal. Adding a new executor (Python, Deno) changes only the Executor union. Adding a new workflow primitive (All, Try) changes only the Action union.

### Executors

```ts
// Internal serialized form (what goes to Rust after Handler resolution)
type Executor =
  | { kind: "Bash"; script: string }
  | { kind: "TypeScript"; path: string; exportedAs?: string; stepConfig?: unknown; valueSchema?: unknown }
```

The TypeScript executor is never written directly by users — it's produced internally by `fromConfig` when resolving `Handler` objects. Users write `createHandler()` and import handlers into configs. See `refactors/past/OPAQUE_HANDLER.md`.

Executors never appear at the action level. They are always wrapped in a workflow primitive (Unit).

### Workflow primitives

Each primitive is irreducible — it cannot be expressed as a combination of other primitives.

| Primitive | Semantics | Why irreducible |
|-----------|-----------|-----------------|
| **Unit** | Execute a single handler. Takes an executor, runs it, returns its output. | The atom of computation. Without it, there's no way to actually do work. |
| **Sequence** | Run actions in order. Each action's output feeds the next's input. | Sequential composition is irreducible. No other primitive gives "A then B with A's result". |
| **All** | Run actions in parallel. Wait for all to complete. Collect results. | Parallel composition is irreducible. No combination of sequential primitives produces concurrency. |
| **Try** | Run an action. On success: `{ kind: "Ok", value }`. On failure: `{ kind: "Err", error }`. | Error observation is irreducible. Without it, failures are invisible — the only responses are retry or drop. |
| **Step** | Dispatch a task to a named step. Wait for that task and all its recursive descendants. Return the result. | Named invocation is irreducible. Without it, there's no way to reference the step graph from within a composition. |

### What's NOT a primitive

**Race / first-to-complete.** Timeouts are per-step config, not a composition operator. Non-deterministic choice adds cancellation semantics for a pattern that's rare in workflow land.

**Map / fan-out.** Derivable. A handler returns `[{kind: "Process", value: item1}, {kind: "Process", value: item2}]` and the engine fans out. Dynamic parallelism is already in the data model.

**Conditional / match.** Branching is expressed by handler return values + the step graph. A handler returns `{kind: "Fix", ...}` or `{kind: "Done", ...}` and the engine routes. The handler already decides.

**Compensation / saga.** Expressible as `Sequence(Try(Step("charge")), route-based-on-result)`. Not a primitive.

**Wait / signal.** External event ingestion (webhooks, human approval, timers). Possibly orthogonal to action composition — but the concurrency slot problem (see "Speculation: Suspend" below) suggests this isn't fully settled.

### The complete types

```rust
/// How computation runs. Always wrapped in a workflow primitive.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum Executor {
    Bash(BashAction),
    TypeScript(TypeScriptAction),
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
  | { kind: "Bash"; script: string }
  | { kind: "TypeScript"; path: string; exportedAs?: string;
      stepConfig?: unknown; valueSchema?: unknown }

// User-facing executor input (what users write in configs)
type ExecutorInput =
  | { kind: "Bash"; script: string }
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
      { kind: "Unit", executor: { kind: "Bash", script: `jq 'if .failedFiles | length > 0
        then [{kind: "Fix", value: .}]
        else [{kind: "Done", value: {}}]
        end'` } },
    ],
  },
  next: ["Fix", "Done"],
}
```

### Finally / cleanup after subtree

Current `finally` is a special hook. With primitives, it's just a Sequence that waits for subtrees then runs cleanup.

```ts
import processHandler from "./process.js";
import cleanupHandler from "./cleanup.js";

// Current: special hook (finally only supports Bash today)
{
  name: "Process",
  action: { kind: "Unit", executor: processHandler },
  finally: { kind: "Bash", script: "./cleanup.sh" },
  next: ["SubtaskA", "SubtaskB"],
}

// With primitives: Sequence + All + Step
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

This is strictly more powerful than `finally`: cleanup gets the subtree results, you can have multiple wait points, and you can compose with Try.

### Error recovery

Step fails → catch the error → route to recovery instead of retrying. Inline Bash is appropriate here — the routing logic is pure data transformation that doesn't warrant a separate handler file.

```ts
{
  name: "SafeProcess",
  action: {
    kind: "Sequence",
    actions: [
      { kind: "Try", action: { kind: "Step", step: "RiskyWork" } },
      { kind: "Unit", executor: { kind: "Bash", script: `jq 'if .kind == "Ok"
        then [{kind: "Continue", value: .value}]
        else [{kind: "Recover", value: .error}]
        end'` } },
    ],
  },
  next: ["Continue", "Recover"],
}
```

### Terminal step (no-op)

Currently requires a Bash hack (`echo '[]'`). This should not need a subprocess. Options:

1. **Optional executor on Unit.** `{ kind: "Unit" }` with no executor returns `[]` (standalone) or passes input through (in a Sequence). Cleanest user-facing API.
2. **Noop executor.** `{ kind: "Unit", executor: { kind: "Noop" } }`. Explicit but verbose.
3. **Dedicated Terminal action.** `{ kind: "Terminal" }`. A workflow primitive, not an executor — it means "this step consumes tasks and produces nothing."

Option 1 is the most likely winner. Open question.

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
  kind: "routing";
  stepConfigValidator: z.ZodType<C>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V>;
  handle: (context: HandlerContext<C, V>) => Promise<FollowUpTask[]>;
}

interface ValueHandler<C = unknown, V = unknown> {
  kind: "value";
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
- Update existing configs: `{ kind: "Bash", script: "..." }` → `{ kind: "Unit", executor: { kind: "Bash", script: "..." } }` and `handler` → `{ kind: "Unit", executor: handler }`

### Ship with JS rewrite

**All**, **Try**, **Step**. These require the action to *suspend* — wait for subtrees to complete before continuing. In the Rust subprocess model, actions are fire-and-forget. In a JS runtime, these are just `await` calls.

- **All**: `await Promise.all(actions.map(run))`
- **Try**: `try { await run(action) } catch (e) { ... }`
- **Step**: `await dispatch(stepName, value)` — dispatch a task to the named step, wait for its entire subtree

Once All + Step ship, `finally` becomes a pattern (Sequence + All + Step) and can be removed as a special-case hook.

## Changes required for Unit + Sequence

### Rust side (`crates/barnum_config`)

**config.rs** — Split `ActionKind` into two enums:

```rust
/// How computation runs. Always wrapped in a workflow primitive.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum Executor {
    Bash(BashAction),
    TypeScript(TypeScriptAction),
}

/// How computations compose.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum Action {
    /// Execute a single handler.
    Unit { executor: Executor },
    /// Run actions in order, piping each one's output to the next's input.
    /// Only the final action's output is parsed as follow-up tasks.
    Sequence(SequenceAction),
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SequenceAction {
    /// The actions to run in order. Must contain at least one action.
    pub actions: Vec<Action>,
}
```

**config.rs validation** — Sequence must have at least one action. Empty sequence is a config error.

**runner dispatch** — Two match arms on Action:

```
match action:
  Unit { executor } ->
    match executor:
      Bash(bash)     -> spawn bash, pipe stdin, return stdout
      TypeScript(ts) -> spawn ts handler, pipe stdin, return stdout
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

2. **Sequence + finally.** Does `finally` still make sense on a step with a Sequence action? Yes — `finally` runs after the task and its descendants complete, regardless of internal structure. Once All + Step ship, `finally` is removed entirely.

3. **Sequence + retries.** If any action in a sequence fails, does the entire sequence retry from the beginning? Yes — the sequence is one atomic action from the retry system's perspective.

4. **Sequence + timeout.** The step-level timeout applies to the entire sequence execution, not individual actions. The sequence is one unit of work.

5. **Unit without executor.** Should `Unit` allow an optional executor, defaulting to a passthrough/no-op? This would make terminal steps cleaner: `{ kind: "Unit" }` instead of a Bash `echo '[]'` hack. The no-executor Unit would pass its input through unchanged (in a Sequence) or produce `[]` (as a standalone step action). See "Terminal step" section above.

## Speculation: is Bash special?

Every place Bash appears in this doc falls into one of two categories:

1. **Real shell work.** Running `tsc`, `cargo test`, calling external tools. The shell is the right tool — you need process spawning, pipes, exit codes.

2. **Inline data routing.** The `jq 'if .kind == "Ok" then ...'` patterns in Pipeline and Error Recovery. These are pure data transformations: take JSON in, produce JSON out. They use Bash only because it's the inline executor we have, not because they need a shell.

Category 2 is suspicious. The routing logic is JavaScript-shaped (conditional on a field, map to a new structure), but it's written in jq-inside-Bash because that's what's available inline. Every routing snippet spawns a subprocess and shells out to jq for what amounts to an `if` statement.

In a TypeScript config file, the natural inline expression is a function:

```ts
// Today: Bash + jq subprocess
{ kind: "Bash", script: `jq 'if .kind == "Ok" then [{kind: "Continue", value: .value}] else [{kind: "Recover", value: .error}] end'` }

// Alternative: inline function (no subprocess)
(input) => input.kind === "Ok"
  ? [{ kind: "Continue", value: input.value }]
  : [{ kind: "Recover", value: input.error }]
```

The inline function is shorter, type-checkable, and doesn't spawn a process. The question is how it fits into the Executor model.

### What would this look like?

The Executor union currently has Bash and TypeScript. An inline JS executor wouldn't be a third variant — it's fundamentally different. Bash and TypeScript are *serializable* — they go over the wire to Rust as JSON. A JavaScript closure can't be serialized.

This means inline functions only work in a JS-native engine where `fromConfig` can capture the closure directly, not in the current Rust subprocess model. Two possible designs:

**Option A: Inline executor (JS engine only).** Add a `{ kind: "Inline", fn: (input) => output }` executor variant that only exists in the user-facing TypeScript types, never serialized. `fromConfig` captures the closure; the JS engine calls it directly. Rust never sees it.

**Option B: Functions as first-class actions.** Don't wrap it in an executor at all. In the user-facing Action type, allow a bare function wherever an Action is expected:

```ts
type ActionInput =
  | { kind: "Unit"; executor: ExecutorInput }
  | { kind: "Sequence"; actions: ActionInput[] }
  | ((input: unknown) => unknown)  // inline transform
  | ...
```

Option B is more ergonomic but blurs the Executor/Action distinction. Option A keeps the levels separate.

### Implications

If inline JS replaces most Bash routing, then Bash's role shrinks to "run external processes" — which is exactly what TypeScript handlers already do (via `child_process`). The remaining Bash use cases are:

- One-liners that are genuinely simpler as shell (`cat`, `echo`, pipes)
- Sandboxed environments where the handler doesn't have filesystem access but the Bash executor does
- Users who prefer shell scripting

None of these are load-bearing for the architecture. Bash isn't a fundamental executor kind — it's a convenience for quick scripts. The two fundamental executor kinds might be "run a handler" (TypeScript/Python/etc.) and "evaluate an expression" (inline transform).

### The closure problem

Inline functions can close over local variables:

```ts
const threshold = config.threshold; // local binding

BarnumConfig.fromConfig({
  steps: [{
    name: "Route",
    action: {
      kind: "Sequence",
      actions: [
        { kind: "Unit", executor: handler },
        // This closure captures `threshold` — breaks if serialized
        (input) => input.score > threshold
          ? [{ kind: "Pass", value: input }]
          : [{ kind: "Fail", value: input }],
      ],
    },
  }],
});
```

This type-checks. But if the function is serialized (sent to Rust, persisted for resume, executed in a different process), `threshold` doesn't exist in the new context. The function silently breaks.

Bash doesn't have this problem because Bash scripts are strings — they're self-contained by construction. A Bash script can't accidentally capture a JavaScript variable.

For inline JS to be safe, we need a guarantee that the function doesn't close over anything that won't be available at execution time. Options:

1. **Static analysis.** Walk the function's AST and verify all free variables are globals (or module-level imports). This is a build step or a `fromConfig`-time check. Doable but adds complexity — need a JS parser, need to define what counts as "global."

2. **Runtime detection.** Serialize the function to a string (`fn.toString()`), execute it in a clean scope, and check if it throws `ReferenceError`. Fragile — a closure might not reference the captured variable on every code path. Missing coverage means silent bugs.

3. **Restricted syntax.** Inline functions must be arrow expressions with no free variables. The config API wraps them in a helper that validates at construction time: `inline((input) => ...)`. The helper can do `fn.toString()` analysis on the source text. Narrows the problem but doesn't eliminate it — `toString()` doesn't always produce parseable source.

4. **Don't serialize them.** Inline functions only work in the in-process JS engine, never cross a serialization boundary. If the engine is pure JS (no Rust subprocess), the closure executes in the same process that created it — no serialization needed. The closure problem only exists when crossing process boundaries. This is the cleanest answer but limits inline functions to the JS engine.

None of these are perfect. Option 4 is the most honest: inline functions are a JS-engine-only feature. In the Rust subprocess model, use Bash or handlers. When the JS engine ships, closures just work because there's no serialization boundary.

The deeper issue: if a suspended task resumes after a restart, and the inline function was a closure, the captured variables are gone. Even Option 4 doesn't help here — the process is different. Durable execution + inline closures is fundamentally at odds. Any function that survives a restart must be self-contained.

### Symmetric inline/external model

Today the two executor kinds are asymmetric:

- **Bash**: always inline (script string in the config). No way to point to an external `.sh` file.
- **TypeScript**: always external (handler file via `createHandler`). No way to write inline JS in the config.

A consistent model would give both languages both forms:

| | Inline | External |
|---|---|---|
| **Bash** | `{ kind: "Bash", script: "jq ..." }` (current) | `{ kind: "Bash", path: "./process.sh" }` |
| **TypeScript** | inline function (see above) | `createHandler({...})` (current) |

External Bash (`path` field) would run the file as a shell script with the same stdin/stdout conventions as inline Bash. This is useful for non-trivial shell scripts that don't belong inline in a config — linting scripts, build scripts, cleanup jobs.

The Executor enum becomes:

```ts
type Executor =
  | { kind: "Bash"; script: string }          // inline bash
  | { kind: "Bash"; path: string }            // external bash file
  | { kind: "TypeScript"; path: string; ... } // external TS handler (from Handler resolution)
  // inline TS: only in JS engine, see closure problem above
```

Or, abstracting over the language:

```ts
type Executor =
  | { kind: "Inline"; language: "bash" | "javascript"; source: string }
  | { kind: "External"; path: string; ... }
```

The second form suggests that the language isn't what matters — what matters is inline vs external. An inline executor is a string of code. An external executor is a file. The language is a property of the executor, not a separate kind.

This also opens the door to other languages (Python, Deno) without adding new top-level kinds — just new `language` values on the same Inline/External executors.

### Don't act on this yet

This is speculative. The current Bash executor works, and the Rust subprocess model requires serializable executors. Inline functions only make sense once the JS engine exists, and even then the closure problem constrains where they can be used. The jq routing patterns in this doc are workarounds for the absence of inline JS, not inherent features of the architecture — but the workarounds are at least safe by construction.

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

The engine sees the Suspend return, writes the task to the state log as suspended, frees the slot. An external signal (`barnum resume <task-id> --value '{approved: true}'`) re-queues the task with the new value. The same handler runs again, this time with the approval in the value.

Pro: no change to the action AST. The handler has context to decide *when* to suspend. Simpler engine changes — suspended is just another task state alongside pending/running/completed.

Con: the handler runs twice (once to suspend, once to resume). It must be idempotent or check state to know it's resuming. The "run the same handler again with new data" pattern is ad-hoc.

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
