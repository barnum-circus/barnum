# Convert Pool Actions to Bash

**Parent:** TS_CONFIG.md
**Depends on:** ACTION_PARAMS_NESTING (landed)

## Motivation

Pool is not a fundamental action kind — it's a bash command that calls `troupe submit_task`. Every Pool action in the existing demo configs can be rewritten as a Bash action whose inline script builds the troupe payload and submits it. This eliminates Pool as a config-level concept before removing it from Rust.

## Scope

Convert all 14 Pool action occurrences across 5 demo config files to Bash actions. Also convert the 5 demo config `.json` files (the non-JSONC duplicates). Update demo.sh scripts that inject pool config.

Affected configs:
- `demos/simple/config.jsonc` — 1 Pool action
- `demos/linear/config.jsonc` — 3 Pool actions
- `demos/branching/config.jsonc` — 4 Pool actions
- `demos/fan-out/config.jsonc` — 2 Pool actions
- `demos/refactor-workflow/config.jsonc` — 4 Pool actions

Plus corresponding `.json` files.

## Conversion

A Pool action like:

```json
{
  "kind": "Pool",
  "params": {
    "instructions": {"kind": "Inline", "value": "Make a decision. Choose PathA or PathB."},
    "pool": null,
    "root": null,
    "timeout": null
  }
}
```

Becomes a Bash action whose script calls `troupe submit_task`:

```json
{
  "kind": "Command",
  "params": {
    "script": "TASK=$(cat); ${TROUPE:-pnpm dlx @barnum/troupe} submit_task --pool $BARNUM_POOL --root $BARNUM_ROOT --notify file --data \"$(jq -n --arg inst 'Make a decision. Choose PathA or PathB.' --argjson task \"$TASK\" '{kind: \"Task\", task: {instructions: $inst, data: $task}}')\""
  }
}
```

The instructions are embedded in the script string. Pool name and root come from environment variables (`$BARNUM_POOL`, `$BARNUM_ROOT`) — demo.sh sets these before running barnum. `$TROUPE` defaults to `pnpm dlx @barnum/troupe` so configs work without demo.sh setup.

Note: this keeps `"kind": "Command"` (the current Rust name) rather than renaming to `"Bash"`. The rename happens in REMOVE_POOL_ACTION.md alongside the Rust changes.

## Step docs

Currently, Rust's `generate_step_docs` wraps the raw instructions with step metadata (step name, valid next steps, response format). For the Bash conversion, the step docs are pre-generated and embedded in the script's instructions argument. Each converted config includes the full step docs text, not just the raw instructions.

The demo configs already have explicit response format instructions in the `value` field (e.g., "Return one of: `[{\"kind\": \"PathA\"...}]`"), so the conversion mostly preserves the existing text with the standard preamble prepended.

## demo.sh changes

The `inject_pool_config` jq function in demo.sh currently patches Pool actions with root and pool:

```bash
jq --arg root "$1" --arg pool "$2" \
    '.steps |= map(if .action.kind == "Pool" then .action.params.root = $root | .action.params.pool = $pool else . end)' \
    "$3"
```

After conversion, Pool actions no longer exist. demo.sh instead sets environment variables:

```bash
export BARNUM_POOL="$POOL_ID"
export BARNUM_ROOT="$POOL_ROOT"
export TROUPE="${TROUPE:-$WORKSPACE_ROOT/target/debug/troupe}"
```

The bash scripts in the configs read these env vars. The `inject_pool_config` function is deleted.

## Test helper

`crates/barnum_config/tests/common/mod.rs` has helper functions that create Pool actions for test configs. These stay unchanged in this refactor — they're addressed in REMOVE_POOL_ACTION.md when Pool is removed from Rust.
