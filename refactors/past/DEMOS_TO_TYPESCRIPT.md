# Convert Demos and Tests to TypeScript Configs

**Depends on:** FLATTEN_AND_RENAME_ACTION

## Motivation

Every demo has three config representations: `config.jsonc` (human-readable), `config.json` (machine-readable copy), and `run-demo.ts` (loads the JSON and calls `.run()`). The JSONC and JSON files are redundant with each other. The `demo.sh` scripts duplicate pool setup logic.

The target: each demo is a single `barnum.config.ts` file that defines the config inline and calls `.run()`. No JSONC, no JSON, no shell wrapper. The TypeScript file IS the demo. Users run `tsx crates/barnum_cli/demos/simple/barnum.config.ts`.

All demos continue to use `kind: "Bash"` actions — this refactor changes the config format (JSONC to inline TypeScript), not the action kind. The TypeScript action kind (ADD_TYPESCRIPT_ACTION) comes later and is independent.

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
        kind: "Bash",
        script: `TASK=$(cat); \${TROUPE:-pnpm dlx @barnum/troupe} submit_task --pool $BARNUM_POOL --root $BARNUM_ROOT --notify file --data "$(jq -n --arg inst 'This is the starting step. Return an empty array to finish.' --argjson task "$TASK" '{task: $task, instructions: $inst}')" | jq -r '.stdout'`,
      },
      next: [],
    },
  ],
}).run()
  .on("exit", (code) => process.exit(code ?? 1));
```

The Bash scripts are the same as before — this refactor changes how the config is defined (inline TypeScript instead of JSONC files), not what the actions do.

### 2. Delete redundant files

For each demo, delete:
- `config.json` — replaced by inline TypeScript config
- `config.jsonc` — same
- `run-demo.ts` — replaced by `barnum.config.ts`

Keep `demo.sh` for now — it handles pool setup and agent lifecycle, which the TypeScript config doesn't manage. `demo.sh` changes to invoke `tsx barnum.config.ts` instead of `$BARNUM run --config config.json`.

### 3. Update demo.sh scripts

Each `demo.sh` currently runs:
```bash
$BARNUM run --config "$SCRIPT_DIR/config.json"
```

Changes to:
```bash
pnpm dlx tsx "$SCRIPT_DIR/barnum.config.ts"
```

The TypeScript file calls `.run()` internally, which spawns the barnum binary.

### 4. Keep config.jsonc as documentation (optional)

The JSONC files serve as readable documentation of the config shape. If we want to preserve them as reference, they stay but are not loaded by anything. Otherwise, delete them — the TypeScript config IS the documentation.

### 5. CLI integration tests

The CLI tests in `crates/barnum_cli/tests/` construct configs as JSON strings and invoke the barnum binary via subprocess. These tests remain JSON-based — they test the Rust CLI's ability to parse and run JSON configs. Converting them to TypeScript would test the JS layer, not Rust.

Tests that need updating:
- JSON field names change from snake_case to camelCase (from FLATTEN_AND_RENAME_ACTION)
- `"kind": "Command"` → `"kind": "Bash"` (from FLATTEN_AND_RENAME_ACTION)
- Action shape flattens: remove `"params"` wrapper (from FLATTEN_AND_RENAME_ACTION)

These test changes happen in FLATTEN_AND_RENAME_ACTION, not here.

### 6. Config crate tests

The `barnum_config` tests construct configs in Rust using struct literals. No TypeScript involved. They continue to work as-is after FLATTEN_AND_RENAME_ACTION updates the struct/variant names.

## Per-demo conversion

All demos keep their existing `Bash` actions — the inline shell scripts are unchanged. The conversion is purely structural: JSONC file → inline TypeScript object.

### simple
- Single terminal step with Bash action.

### linear
- Three-step chain: Start → Middle → End. Three Bash actions.

### branching
- Decision step fans to PathA or PathB, both converge to Done. Shows `next: ["PathA", "PathB"]`.

### fan-out
- Distribute step spawns 20 Worker tasks. All Bash actions.

### command / command-script
- Bash actions demonstrating data transformation with jq.

### hooks
- Demonstrates `finally` hooks. Bash actions for both the main action and the finally hook.

### refactor-workflow
- Complex multi-step workflow with self-loops. Bash actions that read instructions from .md files.

## What this does NOT do

- Does not change action kinds — all demos remain `Bash` actions (TypeScript action kind is ADD_TYPESCRIPT_ACTION)
- Does not change the Rust CLI or barnum_config crate (beyond what FLATTEN_AND_RENAME_ACTION handles)
- Does not add new TypeScript library APIs (BarnumConfig.fromConfig and .run() already exist)
- Does not remove demo.sh scripts — they manage pool/agent lifecycle which is orthogonal
