# Entrypoint Refactor

**Status: Completed** (2026-03-06, commit 02a318c)

## Motivation

Currently, running `gsd run` requires `--initial` with a full JSON array of tasks:

```bash
gsd run config.json --initial '[{"kind": "Analyze", "value": {"path": "/foo"}}]'
```

This is verbose for the common case where you just want to kick off a single task at a known entry point. An `entrypoint` field would allow:

```bash
gsd run config.json --entrypoint-value '{"path": "/foo"}'
# Or even simpler:
gsd run config.json  # Uses {} as the value
```

## Implementation Summary

### Changes Made

1. **Added `entrypoint` field to Config** (`crates/gsd_config/src/config.rs`)
   - Optional `entrypoint: Option<StepName>` field
   - Validation that entrypoint references an existing step
   - New error variant `ConfigError::InvalidEntrypoint`

2. **Renamed CLI flag** (`crates/gsd_cli/src/main.rs`)
   - `--initial` renamed to `--initial-state`
   - Added `--entrypoint-value` for providing initial value to entrypoint step

3. **Resolution logic** (`crates/gsd_cli/src/main.rs:resolve_initial_tasks`)
   - If config has `entrypoint`: use it with `--entrypoint-value` (defaults to `{}`)
   - If config has `entrypoint` and `--initial-state` provided: error E062
   - If no `entrypoint`: require `--initial-state`
   - If `--entrypoint-value` without `entrypoint`: error E063
   - Validates entrypoint value against step's schema

4. **Updated all demos and docs** to use `--initial-state`

### Error Codes

- E060: Invalid `--entrypoint-value` JSON
- E061: Entrypoint value validation failed (schema mismatch)
- E062: Config has entrypoint, use `--entrypoint-value` instead of `--initial-state`
- E063: `--entrypoint-value` requires config to have an entrypoint
- E064: `--initial-state` is required when config has no entrypoint

## Example Usage

Config with entrypoint:

```json
{
  "entrypoint": "Analyze",
  "steps": [
    {
      "name": "Analyze",
      "value_schema": {"type": "object", "properties": {"path": {"type": "string"}}},
      "next": ["Process"]
    },
    {"name": "Process", "next": []}
  ]
}
```

Run options:

```bash
# Using entrypoint with value
gsd run config.json --pool mypool --entrypoint-value '{"path": "/foo"}'

# Using entrypoint with empty object (if schema allows)
gsd run config.json --pool mypool

# Without entrypoint, explicit initial state required
gsd run config.json --pool mypool --initial-state '[{"kind": "Analyze", "value": {"path": "/foo"}}]'
```

## Deviations from Original Plan

- Renamed `default` to `entrypoint` (one word, lowercase)
- Renamed `--initial` to `--initial-state` for clarity
- Renamed `--initial-value` to `--entrypoint-value` for consistency
