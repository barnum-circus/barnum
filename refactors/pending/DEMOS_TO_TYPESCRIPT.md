# Convert Demos and Tests to TypeScript Configs

**Depends on:** FLATTEN_AND_RENAME_ACTION, ADD_TYPESCRIPT_ACTION

## Motivation

Every demo has three config representations: `config.jsonc` (human-readable), `config.json` (machine-readable copy), and `run-demo.ts` (loads the JSON and calls `.run()`). The JSONC and JSON files are redundant with each other and will become redundant with the TypeScript config once ADD_TYPESCRIPT_ACTION lands. The `demo.sh` scripts duplicate pool setup logic.

The target: each demo is a single `barnum.config.ts` file that defines the config inline and calls `.run()`. No JSONC, no JSON, no shell wrapper. The TypeScript file IS the demo. Users run `tsx crates/barnum_cli/demos/simple/barnum.config.ts`.

CLI integration tests similarly construct configs as JSON strings. After this refactor, the Rust tests still use JSON (they test the Rust binary), but the demo configs are TypeScript-first.

## Current state

Each demo directory contains:

| File | Purpose |
|------|---------|
| `config.jsonc` | Human-readable config with comments |
| `config.json` | Same config without comments (for `require()`) |
| `demo.sh` | Shell script: builds binaries, creates pool, starts agent, runs barnum |
| `run-demo.ts` | Loads `config.json`, calls `BarnumConfig.fromConfig(...).run()` |

The `run-demo.ts` files are all identical (12 lines). They load JSON and pass it through — they don't use TypeScript config features at all.

Demos: `simple`, `linear`, `branching`, `fan-out`, `command`, `command-script`, `hooks`, `refactor-workflow`.

## Changes

### 1. Replace run-demo.ts + config.json with barnum.config.ts

Each demo gets a `barnum.config.ts` that defines the config inline:

**Before** (`simple/run-demo.ts`):
```typescript
import { BarnumConfig } from "@barnum/barnum";
import { createRequire } from "node:module";
const require = createRequire(import.meta.url);
BarnumConfig.fromConfig(require("./config.json"))
  .run()
  .on("exit", (code) => process.exit(code ?? 1));
```

**After** (`simple/barnum.config.ts`):
```typescript
import { BarnumConfig } from "@barnum/barnum";

BarnumConfig.fromConfig({
  entrypoint: "Start",
  steps: [
    {
      name: "Start",
      action: {
        kind: "TypeScript",
        path: "./handlers/start.ts",
        stepConfig: {
          instructions: "This is the starting step. Return an empty array to finish.",
        },
      },
      next: [],
    },
  ],
}).run()
  .on("exit", (code) => process.exit(code ?? 1));
```

Demos that currently use Bash `Command` actions with inline troupe `submit_task` scripts convert to TypeScript actions with handler files. This is the whole point — the TypeScript handler interface replaces the gnarly jq/troupe shell one-liners.

### 2. Create shared handler for troupe-backed steps

Most demo steps follow the same pattern: submit a task to a troupe pool with instructions, get back follow-up tasks. A shared handler captures this:

**File:** `crates/barnum_cli/demos/handlers/troupe-step.ts`

```typescript
import { z } from "zod";
import type { HandlerDefinition } from "@barnum/barnum";
// troupe client import TBD

export default {
  stepConfigValidator: z.object({
    instructions: z.string(),
    pool: z.string(),
  }),

  async handle({ stepConfig, value }) {
    // Submit task to troupe pool with instructions
    // Return follow-up tasks from agent response
  },
} satisfies HandlerDefinition;
```

The exact troupe submission mechanism (subprocess call to `troupe submit_task` vs a JS client) is an implementation detail. The handler encapsulates it.

### 3. Delete redundant files per demo

For each demo, delete:
- `config.json` — replaced by inline TypeScript config
- `config.jsonc` — same
- `run-demo.ts` — replaced by `barnum.config.ts`

Keep `demo.sh` for now — it handles pool setup and agent lifecycle, which the TypeScript config doesn't manage. `demo.sh` changes to invoke `tsx barnum.config.ts` instead of `$BARNUM run --config config.json`.

### 4. Update demo.sh scripts

Each `demo.sh` currently runs:
```bash
$BARNUM run --config "$SCRIPT_DIR/config.json"
```

Changes to:
```bash
pnpm dlx tsx "$SCRIPT_DIR/barnum.config.ts"
```

The TypeScript file calls `.run()` internally, which spawns the barnum binary.

### 5. Keep config.jsonc as documentation (optional)

The JSONC files serve as readable documentation of the config shape. If we want to preserve them as reference, they stay but are not loaded by anything. Otherwise, delete them — the TypeScript config IS the documentation.

### 6. Demos that use Bash actions

Some demos (`command`, `command-script`, `hooks`) demonstrate Bash-specific patterns (jq piping, shell scripts). These keep `kind: "Bash"` actions in their TypeScript configs — the point is to show both action kinds.

The `hooks` demo also demonstrates `finally` hooks, which remain Bash actions (finally hooks don't use TypeScript handlers).

### 7. CLI integration tests

The CLI tests in `crates/barnum_cli/tests/` construct configs as JSON strings and invoke the barnum binary via subprocess. These tests remain JSON-based — they test the Rust CLI's ability to parse and run JSON configs. Converting them to TypeScript would test the JS layer, not Rust.

Tests that need updating:
- JSON field names change from snake_case to camelCase (from FLATTEN_AND_RENAME_ACTION)
- `"kind": "Command"` → `"kind": "Bash"` (from FLATTEN_AND_RENAME_ACTION)
- Action shape flattens: remove `"params"` wrapper (from FLATTEN_AND_RENAME_ACTION)

These test changes happen in FLATTEN_AND_RENAME_ACTION, not here.

### 8. Config crate tests

The `barnum_config` tests construct configs in Rust using struct literals. No TypeScript involved. They continue to work as-is after FLATTEN_AND_RENAME_ACTION updates the struct/variant names.

## Per-demo conversion

### simple
- Single terminal step. Convert inline troupe command to TypeScript handler.

### linear
- Three-step chain: Start → Middle → End. Each step becomes a TypeScript action pointing to the shared troupe handler with different instructions.

### branching
- Decision step fans to PathA or PathB, both converge to Done. Same handler, different instructions per step. Shows `next: ["PathA", "PathB"]`.

### fan-out
- Distribute step spawns 20 Worker tasks. The Distribute step uses a Bash action (jq to generate the array), Workers use TypeScript handlers.

### command / command-script
- These demos exist to showcase Bash/Command actions specifically. Keep them as Bash actions in the TypeScript config. They demonstrate `kind: "Bash"` alongside `kind: "TypeScript"`.

### hooks
- Demonstrates `finally` hooks. The main action converts to TypeScript; the finally hook stays as a Bash action (finally hooks are always Bash).

### refactor-workflow
- Complex multi-step workflow with self-loops. Each step reads instructions from .md files. Convert to TypeScript handlers that read the instruction files.

## What this does NOT do

- Does not change the Rust CLI or barnum_config crate (beyond what FLATTEN_AND_RENAME_ACTION handles)
- Does not add new TypeScript library APIs (BarnumConfig.fromConfig and .run() already exist)
- Does not implement a troupe JS client — the handler shells out to the troupe CLI
- Does not remove demo.sh scripts — they manage pool/agent lifecycle which is orthogonal
