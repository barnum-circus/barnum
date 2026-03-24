# Reusable TypeScript Handlers

## Motivation

TypeScript handlers today are coupled to the workflow they live in. The handler's `handle()` method returns `[{ kind: "NextStep", value: ... }]`, where `kind` is a step name from the config. This means a handler must know the graph topology of the workflow that calls it.

Consider a reusable `ts-check` handler that runs `tsc --noEmit` and returns an array of files with type errors. In one workflow, the next step is "Fix". In another, the next step is "Report". The handler can't be shared between them because it has to hardcode the step name.

The goal is to let TypeScript handlers return a plain value and have the config control where that value goes next.

## Current state

A TypeScript handler looks like this (`crates/barnum_cli/demos/typescript-handler/handler.ts`):

```ts
export default {
  stepConfigValidator,
  getStepValueValidator(_stepConfig) { return stepValueValidator; },
  async handle({ value }) {
    return [{ kind: "Done", value: { greeting: `Hello, ${value.name}!` } }];
    //            ^^^^^^ hardcoded step name
  },
} satisfies HandlerDefinition<StepConfig, StepValue>;
```

The bridge script (`libs/barnum/actions/run-handler.ts`) passes the result straight through:

```ts
const results = await handler.handle(envelope);
process.stdout.write(JSON.stringify(results));
```

The Rust runner reads stdout as a `Vec<Task>` where each task has a `step` (kind) and `value`. It validates that each `step` is in the current step's `next` list.

The coupling chain: handler returns step names -> bridge passes them through -> Rust validates them against `next`. The handler must know the graph.

## Proposed approach: implicit routing with optional map

Two changes work together to break the coupling:

### 1. Implicit single-next routing

When a TypeScript handler returns **a plain value** (any JSON that isn't an array of `{kind, value}` tasks), the engine wraps it as a single follow-up task targeting the step's sole `next` entry.

The handler:
```ts
async handle({ value }) {
  return { failedFiles: ["src/foo.ts"] };  // plain value, not [{kind, value}]
}
```

The config:
```ts
{ name: "Check", action: { kind: "TypeScript", path: "..." }, next: ["Fix"] }
```

The engine sees a plain value and `next` has exactly one entry, so it produces `[{ kind: "Fix", value: { failedFiles: ["src/foo.ts"] } }]`.

If `next` has zero entries (terminal step), the plain value is discarded (equivalent to returning `[]`).

If `next` has multiple entries and no `map` is provided, it's a config validation error: "step 'Check' has multiple next steps but no map to choose between them".

### 2. Optional `map` script for branching

When a step needs conditional routing (multiple entries in `next`), the TypeScript action gains an optional `map` field: a bash script that transforms the handler's output into `[{kind, value}]` tasks.

```ts
{
  name: "Check",
  action: {
    kind: "TypeScript",
    path: "@barnum/ts-check",
    map: "jq 'if .failedFiles | length > 0 then [{kind: \"Fix\", value: .}] else [{kind: \"Done\", value: {}}] end'",
  },
  next: ["Fix", "Done"],
}
```

Execution order:
1. Rust spawns the TypeScript handler subprocess as today
2. Handler returns a plain value to stdout
3. Rust pipes that value into the `map` script's stdin
4. `map` script outputs `[{kind, value}]` on stdout
5. Rust validates the tasks against `next` as usual

The `map` script is where workflow-specific routing logic lives. The handler stays generic.

### Detecting which return convention is used

The bridge script (`run-handler.ts`) needs to distinguish "handler returned a plain value" from "handler returned tasks in the old format". The rule:

- If the return value is an array where every element has both `kind` (string) and `value` properties, treat it as the old `[{kind, value}]` format (backward compat).
- Otherwise, treat it as a plain value for implicit routing.

This is a heuristic, but it covers the practical cases. A handler that wants to return an array of objects that happen to have `kind` and `value` fields as a plain value would need to use `map`, which is the right thing anyway since such a handler is doing something unusual.

Alternatively, and arguably better: we could add a boolean field to `TypeScriptAction` like `returnsValue: true` that opts into the new convention. No heuristic needed. Config is explicit about which convention the handler uses. This is the approach I'd recommend.

## Hypothetical config: reusable ts-check

```ts
import { BarnumConfig } from "@barnum/barnum";

// Workflow 1: check -> fix -> commit
await BarnumConfig.fromConfig({
  entrypoint: "Check",
  steps: [
    {
      name: "Check",
      action: {
        kind: "TypeScript",
        path: "@barnum/handlers-ts-check",
        returnsValue: true,
      },
      next: ["Fix"],
    },
    {
      name: "Fix",
      action: { kind: "Bash", script: "..." },
      next: ["Commit"],
    },
    {
      name: "Commit",
      action: { kind: "Bash", script: "..." },
      next: [],
    },
  ],
}).run();

// Workflow 2: check -> report (different routing, same handler)
await BarnumConfig.fromConfig({
  entrypoint: "Check",
  steps: [
    {
      name: "Check",
      action: {
        kind: "TypeScript",
        path: "@barnum/handlers-ts-check",
        returnsValue: true,
      },
      next: ["Report"],
    },
    {
      name: "Report",
      action: { kind: "Bash", script: "..." },
      next: [],
    },
  ],
}).run();

// Workflow 3: check with conditional branching via map
await BarnumConfig.fromConfig({
  entrypoint: "Check",
  steps: [
    {
      name: "Check",
      action: {
        kind: "TypeScript",
        path: "@barnum/handlers-ts-check",
        returnsValue: true,
        map: `jq 'if .failedFiles | length > 0
               then [{kind: "Fix", value: .}]
               else [{kind: "Done", value: {}}]
               end'`,
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
```

## Changes required

### Rust side (`crates/barnum_config`)

**config.rs** - Add `map` and `returns_value` to `TypeScriptAction`:
```rust
pub struct TypeScriptAction {
    pub path: String,
    pub exported_as: String,
    #[serde(default)]
    pub step_config: serde_json::Value,
    #[serde(default)]
    pub value_schema: Option<serde_json::Value>,
    /// When true, the handler returns a plain value instead of [{kind, value}] tasks.
    /// The engine wraps it as a task targeting the single `next` step,
    /// or pipes it through `map` if present.
    #[serde(default)]
    pub returns_value: bool,
    /// Bash script that transforms the handler's plain output into [{kind, value}] tasks.
    /// Receives the handler's stdout on stdin. Required when returns_value is true
    /// and the step has multiple `next` entries.
    #[serde(default)]
    pub map: Option<String>,
}
```

**config.rs validation** - New validation rules:
- If `returns_value` is false and `map` is set, error: "`map` only applies when `returnsValue` is true"
- If `returns_value` is true, `next` has multiple entries, and `map` is absent, error: "step has multiple next steps but no map"

**runner/mod.rs** - After getting TypeScript handler output:
- If `returns_value` is false: process as today (output is `[{kind, value}]`)
- If `returns_value` is true and `map` is absent: wrap output as `[{kind: next[0], value: output}]`
- If `returns_value` is true and `map` is present: pipe output through map script, use result as `[{kind, value}]`

### TypeScript side (`libs/barnum`)

**types.ts** - The `HandlerDefinition` interface gets a second variant for value-returning handlers:

```ts
export interface ValueHandlerDefinition<C = unknown, V = unknown> {
  stepConfigValidator: z.ZodType<C>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V>;
  handle: (context: HandlerContext<C, V>) => Promise<unknown>;
  //                                               ^^^^^^^ plain value
}
```

Or, more simply, `HandlerDefinition` stays the same and handlers that return plain values just return them. The bridge script doesn't care about the return type; it JSON-serializes whatever comes back. The Rust side handles the routing logic.

**run.ts** - `resolveConfig()` validates the new fields (e.g., if `returnsValue` is true and `next.length > 1` and no `map`, throw early).

### Generated schemas

Regenerate after adding the new fields to `TypeScriptAction`. The JSON schema, Zod types, and CLI schema will pick up `returnsValue` and `map` automatically.

## Open questions

1. **Should `map` apply to Bash actions too?** Right now Bash actions own their routing because they already output `[{kind, value}]`. But there might be value in having a "pure computation" Bash script that returns a value and gets routed by the config. This would make Bash actions equally reusable. If we do this, `map` and `returnsValue` should move from `TypeScriptAction` to `Step` level.

2. **Should `map` be TypeScript instead of Bash?** Bash + jq is the current convention, but for complex routing logic, a TypeScript function would be more ergonomic. Counter-argument: `map` should be a dumb data transform, not complex logic. If it's complex enough to need TypeScript, the handler should probably handle the routing itself.

3. **Naming**: `returnsValue` vs `returnsPlainValue` vs `autoRoute` vs `implicitNext`. The field name should communicate that the handler doesn't manage its own routing.

4. **Fan-out from a value-returning handler**: If a handler returns `{ items: [a, b, c] }` and you want to fan out to three tasks, `map` handles it (`jq '.items | map({kind: "Process", value: .})'`). Is this sufficient or do we want a more structured fan-out primitive?
