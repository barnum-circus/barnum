# Reusable TypeScript Handlers

## Motivation

TypeScript handlers today are coupled to the workflow they live in. The handler's `handle()` method returns `[{ kind: "NextStep", value: ... }]`, where `kind` is a step name from the config. A handler must know the graph topology of the workflow that calls it.

Consider a reusable `ts-check` handler that runs `tsc --noEmit` and returns files with type errors. In one workflow, the next step is "Fix". In another, the next step is "Report". The handler can't be shared between them because it hardcodes the step name in its return value.

The goal: TypeScript handlers return plain values. The config controls routing.

## Current state

A TypeScript handler (`crates/barnum_cli/demos/typescript-handler/handler.ts`):

```ts
export default {
  stepConfigValidator,
  getStepValueValidator(_stepConfig) { return stepValueValidator; },
  async handle({ value }) {
    return [{ kind: "Done", value: { greeting: `Hello, ${value.name}!` } }];
    //            ^^^^^^ hardcoded step name — handler must know the graph
  },
} satisfies HandlerDefinition<StepConfig, StepValue>;
```

The bridge script (`libs/barnum/actions/run-handler.ts`) passes the result through:

```ts
const results = await handler.handle(envelope);
process.stdout.write(JSON.stringify(results));
```

The Rust runner reads stdout as `Vec<Task>` and validates each task's step against the step's `next` list.

The coupling chain: handler returns step names -> bridge passes them through -> Rust validates against `next`. The handler must know the graph.

## Action kind taxonomy

Currently Barnum has two action kinds: `Bash` and `TypeScript`. Thinking about what workflow engines like Temporal and Inngest express, and what primitives are needed for composition:

| Kind | What it does | Status |
|------|-------------|--------|
| **Bash** | Run a shell script. stdin = envelope, stdout = tasks. | Exists |
| **TypeScript** | Run a handler module. stdin = envelope, stdout = tasks. | Exists |
| **Sequence** | Run actions in order, piping data between them. Only the last action produces tasks. | Proposed (this doc) |
| **Try** | Run an action. On success, produce `{ ok: true, value }`. On failure, produce `{ ok: false, error }`. Turns failures into routable data. | Future (JS rewrite) |
| **Parallel** | Run actions concurrently, collect all results into an array. | Future (JS rewrite) |

`Sequence` is the immediate need — it decouples TypeScript handlers from routing. `Try` and `Parallel` are dramatically easier to implement in a JS runtime than in Rust subprocess piping, so they wait for the JS rewrite.

### What's missing from this taxonomy

**Error handling / routing on failure.** Currently a step either succeeds or retries until it drops. There's no way to route to a recovery step on failure. `Try` solves this: wrap an action in `Try`, and failures become a value `{ ok: false, error: "..." }` that the next action in a `Sequence` can inspect and route accordingly. Without `Try`, the only error handling is retries and `finally` hooks.

**Compensation / rollback.** If step 3 fails, undo steps 1 and 2. Temporal has sagas for this. In Barnum's model, `finally` hooks partially cover this (they run after a subtree completes), but they don't have access to the success/failure status of descendants. This is a JS-rewrite concern.

**Wait for external signal.** Pause a workflow until an event arrives (webhook, human approval, timer). Temporal's signals and Inngest's `step.waitForEvent()`. Not in scope for the current Rust engine.

## Proposed approach: Sequence action kind

`Sequence` is a new action kind that contains an ordered list of actions. Each action's stdout feeds the next action's stdin. Only the last action's stdout is parsed as `[{kind, value}]` follow-up tasks.

### Config shape

```ts
{
  name: "Check",
  action: {
    kind: "Sequence",
    actions: [
      { kind: "TypeScript", path: "@barnum/ts-check" },
      { kind: "Bash", script: `jq 'if .failedFiles | length > 0
        then [{kind: "Fix", value: .}]
        else [{kind: "Done", value: {}}]
        end'` },
    ],
  },
  next: ["Fix", "Done"],
}
```

`Bash` and `TypeScript` work exactly as today when used as the sole action on a step. `Sequence` composes them. A `Sequence` with one action is equivalent to that action alone.

### Hypothetical configs

```ts
import { BarnumConfig } from "@barnum/barnum";

// Workflow 1: reusable handler with simple routing
await BarnumConfig.fromConfig({
  entrypoint: "Check",
  steps: [
    {
      name: "Check",
      action: {
        kind: "Sequence",
        actions: [
          { kind: "TypeScript", path: "@barnum/ts-check" },
          { kind: "Bash", script: "jq '[{kind: \"Fix\", value: .}]'" },
        ],
      },
      next: ["Fix"],
    },
    {
      name: "Fix",
      action: { kind: "Bash", script: "..." },
      next: [],
    },
  ],
}).run();

// Workflow 2: same handler, conditional branching
await BarnumConfig.fromConfig({
  entrypoint: "Check",
  steps: [
    {
      name: "Check",
      action: {
        kind: "Sequence",
        actions: [
          { kind: "TypeScript", path: "@barnum/ts-check" },
          {
            kind: "Bash",
            script: `jq 'if .failedFiles | length > 0
              then [{kind: "Fix", value: .}]
              else [{kind: "Done", value: {}}]
              end'`,
          },
        ],
      },
      next: ["Fix", "Done"],
    },
    {
      name: "Fix",
      action: { kind: "Bash", script: "..." },
      next: ["Done"],
    },
    {
      name: "Done",
      action: { kind: "Bash", script: "cat > /dev/null; echo '[]'" },
      next: [],
    },
  ],
}).run();

// Workflow 3: fan-out from a reusable handler
await BarnumConfig.fromConfig({
  entrypoint: "Discover",
  steps: [
    {
      name: "Discover",
      action: {
        kind: "Sequence",
        actions: [
          { kind: "TypeScript", path: "@barnum/ts-check" },
          { kind: "Bash", script: "jq '.failedFiles | map({kind: \"FixFile\", value: {file: .}})'" },
        ],
      },
      next: ["FixFile"],
    },
    {
      name: "FixFile",
      action: { kind: "Bash", script: "..." },
      next: [],
    },
  ],
}).run();
```

## What to ship now vs later

### Ship now (current branch)

The current state is already useful and shippable:
- TypeScript handler support with validation
- Synchronous `run()` API
- Troupe decoupled from barnum_cli

People are using Barnum internally. Ship this, let them switch to the new version.

### Ship soon (Rust, before JS rewrite)

`Sequence` action kind. Requires:
- New `ActionKind::Sequence` variant in `config.rs`
- Piping logic in `runner/mod.rs` (capture intermediate stdout, feed to next action)
- Schema regeneration

The Rust implementation is straightforward: for a Sequence, spawn each action as a subprocess sequentially, capturing stdout and feeding it as stdin to the next. Only the final action's stdout goes through response parsing.

### Ship with JS rewrite

- `Try` — wraps an action, catches failures, produces `{ ok, value/error }`. In JS this is just try/catch around an async function call. In Rust subprocess land it's awkward because "failure" means non-zero exit, stderr parsing, etc.
- `Parallel` — runs actions concurrently. In JS this is `Promise.all`. In Rust it's managing multiple child processes with their own stdio.
- Error routing — step-level `onError` config that routes to a recovery step instead of retrying.
- Compensation — structured undo when downstream steps fail.
- Wait/signal — pause until external event.

## Changes required for Sequence

### Rust side (`crates/barnum_config`)

**config.rs** — New variant:

```rust
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum ActionKind {
    Bash(BashAction),
    TypeScript(TypeScriptAction),
    Sequence(SequenceAction),
}

/// Run a sequence of actions, piping each one's stdout to the next's stdin.
/// Only the final action's stdout is parsed as follow-up tasks.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub struct SequenceAction {
    /// The actions to run in order. Must contain at least one action.
    pub actions: Vec<ActionKind>,
}
```

Note: `SequenceAction.actions` contains `ActionKind`, so a Sequence could theoretically nest another Sequence. Whether to allow or forbid nesting is a validation question. Allowing it is simpler (just flatten at runtime) and causes no harm.

**config.rs validation** — Sequence must have at least one action. Empty sequence is a config error.

**runner/mod.rs** — Dispatch for Sequence:

```
fn dispatch_action(action, envelope_stdin) -> stdout:
  match action:
    Bash(bash)     -> spawn bash, pipe envelope_stdin, return stdout
    TypeScript(ts) -> spawn ts handler, pipe envelope_stdin, return stdout
    Sequence(seq)  ->
      let data = envelope_stdin
      for action in seq.actions[..len-1]:
        data = dispatch_action(action, data)  // capture intermediate stdout
      dispatch_action(seq.actions.last(), data)  // final action: parse as tasks
```

The existing `dispatch_action` already spawns a subprocess and captures stdout. For non-final actions in a sequence, we capture stdout instead of parsing it. For the final action, we parse normally.

### TypeScript side (`libs/barnum`)

**types.ts** — Two handler interfaces coexist:

```ts
// Self-routing handler: returns [{kind, value}] tasks directly
export interface HandlerDefinition<C = unknown, V = unknown> {
  stepConfigValidator: z.ZodType<C, z.ZodTypeDef, unknown>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V, z.ZodTypeDef, unknown>;
  handle: (context: HandlerContext<C, V>) => Promise<FollowUpTask[]>;
}

// Pipeline handler: returns a plain value for the next action to consume
export interface ValueHandlerDefinition<C = unknown, V = unknown, R = unknown> {
  stepConfigValidator: z.ZodType<C, z.ZodTypeDef, unknown>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V, z.ZodTypeDef, unknown>;
  handle: (context: HandlerContext<C, V>) => Promise<R>;
}
```

`HandlerDefinition` is for handlers used as the sole action or as the last action in a sequence. `ValueHandlerDefinition` is for handlers used in non-final positions.

**run.ts** — `resolveConfig()` walks into Sequence actions to find and validate TypeScript handlers.

### Generated schemas

Regenerated. `ActionKind` gains a `Sequence` variant with an `actions` array.

## Open questions

1. **Nested sequences.** Allow `Sequence` inside `Sequence`? Simplest answer: allow it, flatten at runtime. No reason to forbid it, and it means the type system stays simple (actions are always `ActionKind`).

2. **Sequence + finally.** Does `finally` still make sense on a step with a Sequence action? Yes — `finally` runs after the task and its descendants complete, regardless of how the task's action is structured internally.

3. **Sequence + retries.** If the final action in a sequence fails, does the entire sequence retry from the beginning? Yes — the sequence is one atomic action from the retry system's perspective. An intermediate action failing also retries the whole sequence.

4. **Sequence + timeout.** The step-level timeout applies to the entire sequence execution, not individual actions within it. This is correct — the sequence is one unit of work.
