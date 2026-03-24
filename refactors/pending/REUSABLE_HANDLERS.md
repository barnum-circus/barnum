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

## Proposed approach: action pipelines

A step's `action` becomes a pipeline of actions. Each action in the pipeline receives the previous action's stdout on its stdin. Only the final action in the pipeline produces `[{kind, value}]` tasks for routing. All earlier actions return plain values.

### Single action (unchanged)

```ts
{
  name: "Print",
  action: { kind: "Bash", script: "jq -r '.value.message'; echo '[]'" },
  next: [],
}
```

A single action works exactly as today. No change needed for existing configs.

### Pipeline action (new)

```ts
{
  name: "Check",
  action: [
    { kind: "TypeScript", path: "@barnum/ts-check" },
    { kind: "Bash", script: `jq 'if .failedFiles | length > 0
      then [{kind: "Fix", value: .}]
      else [{kind: "Done", value: {}}]
      end'` },
  ],
  next: ["Fix", "Done"],
}
```

When `action` is an array, the engine runs each action in sequence:

1. The first action receives the task envelope on stdin (as today).
2. Each subsequent action receives the previous action's stdout on its stdin.
3. Only the last action's stdout is parsed as `[{kind, value}]` tasks.

The TypeScript handler becomes a pure function: value in, value out. The routing bash script at the end is the only thing that knows about step names.

### Hypothetical config: reusable ts-check

```ts
import { BarnumConfig } from "@barnum/barnum";
import { resolve } from "node:path";

// Workflow 1: check -> fix -> commit (single next, routing is trivial)
await BarnumConfig.fromConfig({
  entrypoint: "Check",
  steps: [
    {
      name: "Check",
      action: [
        { kind: "TypeScript", path: "@barnum/ts-check" },
        { kind: "Bash", script: "jq '[{kind: \"Fix\", value: .}]'" },
      ],
      next: ["Fix"],
    },
    {
      name: "Fix",
      action: { kind: "Bash", script: "..." },
      next: ["Commit"],
    },
    {
      name: "Commit",
      action: { kind: "Bash", script: "cat > /dev/null; echo '[]'" },
      next: [],
    },
  ],
}).run();

// Workflow 2: same handler, different routing
await BarnumConfig.fromConfig({
  entrypoint: "Check",
  steps: [
    {
      name: "Check",
      action: [
        { kind: "TypeScript", path: "@barnum/ts-check" },
        { kind: "Bash", script: "jq '[{kind: \"Report\", value: .}]'" },
      ],
      next: ["Report"],
    },
    {
      name: "Report",
      action: { kind: "Bash", script: "..." },
      next: [],
    },
  ],
}).run();

// Workflow 3: conditional branching
await BarnumConfig.fromConfig({
  entrypoint: "Check",
  steps: [
    {
      name: "Check",
      action: [
        { kind: "TypeScript", path: "@barnum/ts-check" },
        {
          kind: "Bash",
          script: `jq 'if .failedFiles | length > 0
            then [{kind: "Fix", value: .}]
            else [{kind: "Done", value: {}}]
            end'`,
        },
      ],
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

// Workflow 4: fan-out from a handler
await BarnumConfig.fromConfig({
  entrypoint: "Discover",
  steps: [
    {
      name: "Discover",
      action: [
        { kind: "TypeScript", path: "@barnum/ts-check" },
        { kind: "Bash", script: "jq '.failedFiles | map({kind: \"FixFile\", value: {file: .}})'" },
      ],
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

The same `@barnum/ts-check` handler is used in all four workflows with different routing.

## Changes required

### Rust side (`crates/barnum_config`)

**config.rs** — `action` becomes `ActionKind | Vec<ActionKind>`:

```rust
/// A step's action: either a single action or a pipeline of actions.
/// When a pipeline, each action's stdout feeds the next action's stdin.
/// Only the final action's output is parsed as follow-up tasks.
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum Action {
    Single(ActionKind),
    Pipeline(Vec<ActionKind>),
}
```

The `Step` struct changes `action: ActionKind` to `action: Action`.

**config.rs validation** — A pipeline must have at least one action. (An empty pipeline is a config error.)

**runner/mod.rs** — Dispatch logic changes:

- `Action::Single(action)`: dispatch as today.
- `Action::Pipeline(actions)`: run each action in sequence. The first action gets the task envelope on stdin. Each subsequent action gets the previous one's stdout. Only the last action's stdout is parsed as `[{kind, value}]`.

The piping between actions happens in the Rust runner. Each non-final action spawns a subprocess, captures its stdout, and feeds it as stdin to the next subprocess. The final subprocess's stdout goes through the existing response parsing and validation.

**runner/response.rs** — No changes. Response validation only sees the final action's output.

### TypeScript side (`libs/barnum`)

**types.ts** — `HandlerDefinition.handle` return type becomes `Promise<unknown>` instead of `Promise<FollowUpTask[]>`. The handler can return anything JSON-serializable. When used as the last action in a pipeline (or as the only action), the old `[{kind, value}]` format still works. When used as a non-final action in a pipeline, the return value is piped to the next action.

Actually — `FollowUpTask[]` is still the right return type when the handler is the sole action (backward compat). The pipeline is a config-level concern, not a type-level concern. `HandlerDefinition` stays as-is. A handler that's designed for pipeline use just happens to return a plain value, and its type would use a different interface:

```ts
export interface ValueHandlerDefinition<C = unknown, V = unknown, R = unknown> {
  stepConfigValidator: z.ZodType<C>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V>;
  handle: (context: HandlerContext<C, V>) => Promise<R>;
}
```

Both interfaces coexist. `HandlerDefinition` is for self-routing handlers. `ValueHandlerDefinition` is for pipeline-compatible handlers that return plain values.

**run.ts** — `resolveConfig()` iterates pipeline actions the same way it iterates single actions. If any action in the pipeline is `TypeScript`, it imports the handler and validates schemas.

**barnum-config-schema.zod.ts** — Regenerated. The `action` field becomes a union of `ActionKind | ActionKind[]`.

### Generated schemas

Regenerate after changing the `Step.action` type. The JSON schema will show `action` as `oneOf: [ActionKind, array of ActionKind]`.

## How the Rust runner pipes actions

For a pipeline `[A, B, C]` processing task `T`:

1. Spawn `A` with `T`'s envelope on stdin. Capture `A`'s stdout as `output_a`.
2. Spawn `B` with `output_a` on stdin. Capture `B`'s stdout as `output_b`.
3. Spawn `C` with `output_b` on stdin. Parse `C`'s stdout as `[{kind, value}]`.

Each intermediate action runs as a subprocess (same as today), but its stdout is captured instead of parsed as tasks. Only the final action's stdout goes through `process_stdout` / `validate_response`.

If any intermediate action fails (non-zero exit, timeout), the whole pipeline fails and follows the same retry/error logic as a single action failure.

## Open questions

1. **Should non-final actions in a pipeline get the raw handler output, or a wrapped envelope?** The first action gets the standard envelope `{kind, value, config, stepName}`. Subsequent actions get raw stdout from the previous action. This means the second action in a pipeline doesn't get `kind`/`stepName` context — it just gets the data. Is that sufficient?

2. **Should `finally` hooks see the final action's output or the original task?** Currently `finally` gets the original task envelope. With pipelines, the task still has the same `kind` and `value`, so this doesn't change.

3. **Pipeline of length 1** is equivalent to `Action::Single`. Should we canonicalize? Probably yes — `[X]` and `X` should behave identically, and serde's `untagged` enum handles this naturally (it tries `Single` first).
