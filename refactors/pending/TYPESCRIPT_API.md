# TypeScript API for Barnum Workflows

## Motivation

Today, barnum workflows are defined in JSON config files with two action kinds: `Pool` (send to AI agents) and `Command` (run a bash script). The `Command` kind handles deterministic glue logic — fan-out, transformations, filtering — but it's untyped bash scripts communicating via stdin/stdout JSON. There's no type safety, no editor completion, and no compile-time validation that a step returns tasks targeting valid next steps.

A TypeScript API would let users define workflows programmatically with full static typing: the step's value schema becomes a TypeScript type, the return type constrains which steps can be spawned, and the entire workflow graph is checked at compile time.

## Current State

### Action kinds

`crates/barnum_config/src/config.rs:161-187`:
```rust
#[serde(tag = "kind")]
pub enum ActionFile {
    Pool { instructions: MaybeLinked<Instructions> },
    Command { script: String },
}
```

### Command dispatch

`crates/barnum_config/src/runner/dispatch.rs:108-132`:
- Wraps task as `{"kind": "<step>", "value": <payload>}` JSON
- Pipes it to stdin of the shell script
- Parses stdout as JSON array of follow-up tasks

### Hook system

Three hook types (`pre`, `post`, `finally`) all use `{"kind": "Command", "script": "..."}` — bash scripts with JSON stdin/stdout.

## Design Space

There are two meaningfully different approaches here, and they're not mutually exclusive.

### Approach A: TypeScript as a new action kind

Add `Typescript` alongside `Pool` and `Command` as a third action kind. A `.ts` file's default export is a function that takes the typed task value and returns an array of next tasks.

**Config:**
```jsonc
{
  "steps": [
    {
      "name": "Distribute",
      "action": {
        "kind": "Typescript",
        "module": "./steps/distribute.ts"
      },
      "next": ["Worker"]
    }
  ]
}
```

**TypeScript module (`steps/distribute.ts`):**
```typescript
import type { StepHandler } from "@barnum/barnum";

interface DistributeInput {
  files: string[];
}

const handler: StepHandler<DistributeInput> = async (task) => {
  return task.value.files.map(file => ({
    kind: "Worker" as const,
    value: { file },
  }));
};

export default handler;
```

**Runtime:** Barnum spawns a Node/Deno/Bun subprocess (or a long-lived TypeScript process) that loads the module, calls the default export with the task, and returns the result as JSON.

**Pros:**
- Incremental — works alongside existing Command and Pool actions
- Users can adopt it one step at a time
- Minimal config schema changes (just add a new `kind`)

**Cons:**
- Types are manually written, not derived from the config
- No compile-time guarantee that `kind: "Worker"` is actually a valid next step
- Subprocess overhead per invocation (unless long-lived)

### Approach B: TypeScript-first workflow definition

The entire workflow is defined in TypeScript. The config JSON is either generated from the TS definition or not used at all. The TypeScript types enforce the graph structure at compile time.

**TypeScript definition (`workflow.ts`):**
```typescript
import { defineWorkflow, step } from "@barnum/barnum";

// Each step declares its value type and which steps it can spawn
const distribute = step("Distribute")
  .value<{ files: string[] }>()
  .next("Worker")
  .pool({ instructions: "./distribute.md" });

const worker = step("Worker")
  .value<{ file: string }>()
  .next("Report")
  .handler(async (task) => {
    const result = await processFile(task.value.file);
    return [{ kind: "Report", value: { file: task.value.file, result } }];
  });

const report = step("Report")
  .value<{ file: string; result: string }>()
  .terminal()
  .command("./report.sh");

export default defineWorkflow({
  entrypoint: "Distribute",
  steps: [distribute, worker, report],
});
```

**How it works:**
1. User writes workflow in TypeScript
2. `barnum run workflow.ts` (or `barnum build workflow.ts` to emit JSON)
3. The TypeScript definitions compile to the same runtime config Barnum already uses
4. Steps with `.handler()` are TypeScript action kinds; steps with `.pool()` or `.command()` use existing action kinds

**Pros:**
- Full type safety: the return type of a handler is constrained to only valid next steps
- Single source of truth: types and config are the same thing
- Can still emit JSON config for inspection/debugging
- Value schemas are derived from TypeScript types (via something like `zod` or `typebox`)

**Cons:**
- Larger surface area
- Requires a build/compilation step
- Users must understand TypeScript to use it

### Approach C (incremental path): TypeScript as Command replacement, then grow

Start with Approach A (TypeScript action kind) as a drop-in replacement for Command scripts. This gets TypeScript into the system with minimal disruption. Then, in a later phase, add the workflow-definition API (Approach B) on top.

Phase 1: New `Typescript` action kind
- Same stdin/stdout contract as `Command`, but the "script" is a `.ts` module
- Default export is `(task: { kind: string, value: unknown }) => Promise<Array<{ kind: string, value: unknown }>>`
- Barnum runs it via `node --import tsx` or `npx tsx` (for TypeScript execution)
- Types are manually authored by the user, but at least they exist

Phase 2: TypeScript workflow builder
- `@barnum/barnum` exports `defineWorkflow`, `step`, etc.
- The builder API generates the JSON config + wires up TypeScript handlers
- Type inference ensures handlers return valid next-step kinds
- `barnum run workflow.ts` is sugar for "compile TS workflow to config + execute"

## Key Design Decisions

### 1. How does TypeScript get executed?

Options:
- **`npx tsx`** — most portable, works with any Node.js install. Slow cold start.
- **Long-lived TypeScript process** — Barnum starts a single TS process that handles all TypeScript actions via stdin/stdout JSON-RPC. Fast after startup. Adds complexity.
- **Compile to JS first** — `barnum build` transpiles TS to JS, then `node` runs JS. Fast runtime, requires build step.
- **Deno/Bun** — native TS execution, but adds runtime dependency.

Recommendation: Start with `npx tsx` for simplicity. The Command action already spawns a subprocess per invocation, so the model is identical. Optimize later with a long-lived process if the per-invocation overhead matters.

### 2. How do TypeScript types relate to JSON schemas?

Today, `value_schema` is a JSON Schema object in the config. If we're writing TypeScript, we want TypeScript types to be the source of truth.

Options:
- **Manual alignment** — user writes both TS types and JSON schemas. Error-prone.
- **TypeScript → JSON Schema** — use a library like `typebox` or `zod` to define types that emit both TS types and JSON Schema. The builder API calls `.value(Type.Object({ count: Type.Integer() }))` and generates both.
- **JSON Schema → TypeScript** — generate TS types from the existing JSON schemas. Less ergonomic but zero migration cost for existing configs.

Recommendation: Use `typebox` or `zod` in the builder API. For the simple action-kind approach (Phase 1), just use manual TS types with no schema generation.

### 3. What's the handler signature?

```typescript
// Minimal — same contract as Command
type Handler = (input: { kind: string; value: unknown }) => Promise<Array<{ kind: string; value: unknown }>>;

// Typed — with generics constrained by the step definition
type StepHandler<TValue, TNextSteps extends string> = (
  task: { kind: string; value: TValue }
) => Promise<Array<{ kind: TNextSteps; value: unknown }>>;
```

Phase 1 uses the minimal signature. Phase 2 adds the typed version via the builder API.

### 4. Does this replace hooks too?

The three hook types (pre, post, finally) are all `{"kind": "Command", "script": "..."}` today. Should we add `{"kind": "Typescript", "module": "./hooks/pre.ts"}` variants?

Yes, but not in phase 1. Phase 1 is just the action kind. Phase 2 can add TypeScript hooks. The builder API makes hooks natural:

```typescript
const analyze = step("Analyze")
  .pre(async (value) => ({ ...value, timestamp: Date.now() }))
  .pool({ instructions: "./analyze.md" })
  .post(async (outcome) => {
    if (outcome.kind === "Success") {
      // filter follow-ups
    }
    return outcome;
  });
```

## Open Questions

1. **Should the TypeScript action kind use a persistent process or spawn per invocation?** Per-invocation is simpler and matches the Command model. Persistent is faster but adds complexity (process lifecycle, error recovery, warm-up).

2. **What's the minimum viable TypeScript runtime?** Can we assume `node` is on PATH? Should we bundle `tsx`? Should `@barnum/barnum` list `tsx` as a dependency?

3. **Is the builder API (Approach B) the right long-term direction, or should the config JSON remain the source of truth with TypeScript as just another action kind?** This is a philosophical question about whether Barnum is a JSON-config-driven engine with TS bindings or a TypeScript-first framework that happens to support JSON config.

4. **Should TypeScript handlers be async?** The current Command model is synchronous (run script, wait for stdout). Async handlers open up calling external APIs, reading files, etc. But barnum's runner already manages concurrency via `max_concurrency` — async handlers would add a second layer of concurrency inside the handler.
