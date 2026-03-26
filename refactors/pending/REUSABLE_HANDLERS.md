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
| **Try** | Run an action. On success: `{ ok: true, value }`. On failure: `{ ok: false, error }`. | Error observation is irreducible. Without it, failures are invisible — the only responses are retry or drop. |
| **Step** | Dispatch a task to a named step. Wait for that task and all its recursive descendants. Return the result. | Named invocation is irreducible. Without it, there's no way to reference the step graph from within a composition. |

### What's NOT a primitive

**Race / first-to-complete.** Timeouts are per-step config, not a composition operator. Non-deterministic choice adds cancellation semantics for a pattern that's rare in workflow land.

**Map / fan-out.** Derivable. A handler returns `[{kind: "Process", value: item1}, {kind: "Process", value: item2}]` and the engine fans out. Dynamic parallelism is already in the data model.

**Conditional / match.** Branching is expressed by handler return values + the step graph. A handler returns `{kind: "Fix", ...}` or `{kind: "Done", ...}` and the engine routes. The handler already decides.

**Compensation / saga.** Expressible as `Sequence(Try(Step("charge")), route-based-on-result)`. Not a primitive.

**Wait / signal.** External event ingestion (webhooks, human approval, timers). Orthogonal to action composition — it's a runtime concern, not an AST node.

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
// Current: special hook
{
  name: "Process",
  action: { kind: "Unit", executor: { kind: "Bash", script: "..." } },
  finally: { kind: "Bash", script: "./cleanup.sh" },
  next: ["SubtaskA", "SubtaskB"],
}

// With primitives: Sequence + All + Step
{
  name: "Process",
  action: {
    kind: "Sequence",
    actions: [
      { kind: "Unit", executor: { kind: "Bash", script: "..." } },
      { kind: "All", actions: [
        { kind: "Step", step: "SubtaskA" },
        { kind: "Step", step: "SubtaskB" },
      ]},
      { kind: "Unit", executor: { kind: "Bash", script: "./cleanup.sh" } },
    ],
  },
  next: ["SubtaskA", "SubtaskB"],
}
```

This is strictly more powerful than `finally`: cleanup gets the subtree results, you can have multiple wait points, and you can compose with Try.

### Error recovery

Step fails → catch the error → route to recovery instead of retrying.

```ts
{
  name: "SafeProcess",
  action: {
    kind: "Sequence",
    actions: [
      { kind: "Try", action: { kind: "Step", step: "RiskyWork" } },
      { kind: "Unit", executor: { kind: "Bash", script: `jq 'if .ok
        then [{kind: "Continue", value: .value}]
        else [{kind: "Recover", value: .error}]
        end'` } },
    ],
  },
  next: ["Continue", "Recover"],
}
```

### Terminal step (no-op)

Currently requires a Bash hack. With Unit + a no-op executor:

```ts
{
  name: "Done",
  action: { kind: "Unit", executor: { kind: "Bash", script: "echo '[]'" } },
  next: [],
}
```

Note: Unit still requires an executor. A truly empty no-op would be a `{ kind: "Bash", script: "echo '[]'" }` executor inside Unit. This is mildly ugly — we could consider adding a `Noop` executor that returns `[]` without spawning a subprocess, or making the `executor` field optional on Unit (defaulting to the identity/passthrough). Open question.

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

5. **Unit without executor.** Should `Unit` allow an optional executor, defaulting to a passthrough/no-op? This would make terminal steps cleaner: `{ kind: "Unit" }` instead of `{ kind: "Unit", executor: { kind: "Bash", script: "echo '[]'" } }`. The no-executor Unit would pass its input through unchanged (in a Sequence) or produce `[]` (as a standalone step action).
