# TypeScript Step Helpers

**Status:** Stub — deferred

## Motivation

Barnum Command steps are glue logic: read stdin JSON, extract the task value, do some routing/transformation, emit follow-up tasks as JSON to stdout. Every Command step in `r1-3/config.jsonc` does this same pattern in bash with `jq`. The helpers provide TypeScript functions that generate these `StepFile` objects, replacing hand-written bash/jq with typed builders.

## Scope (rough)

### Common patterns from `r1-3/config.jsonc`

The existing workflow has these recurring Command step patterns:

- **Fan-out**: Take an array from the input value, spawn one task per element on a target step. (e.g., distribute branches to individual processing pipelines)
- **Passthrough with condition**: Check some condition (e.g., "is the diff empty?"), either emit a task for the next step or emit `[]` to stop.
- **Conditional routing**: Based on the result of a command, route to one step or another (e.g., validation passes -> Judge, validation fails -> Fix).

### Proposed helpers

Helper functions that return `StepFile` objects:

- `fanOut(name, { arrayField, targetStep, mapItem })` — generates a Command step that maps array elements to tasks
- `passthrough(name, { targetStep })` — forwards the value unchanged to another step
- `conditional(name, { condition, ifTrue, ifFalse })` — routes to different steps based on a predicate
- `commandStep(name, { script, next })` — typed wrapper around a raw Command action

Each helper generates a bash Command script (using `jq`) matching the current execution model. No new runtime dependency is introduced.

### Relationship to runtime decisions

If `TYPESCRIPT_RUNTIME.md` lands with a TS runner, these helpers could generate `node -e` scripts instead of bash/jq, allowing the handler logic to be plain TypeScript. The choice between bash and node for generated Command scripts depends on which runtime is available. This is why the helpers are downstream of the runtime work.

## Dependencies

- `TYPESCRIPT_CONFIG.md` — types must exist so the helpers can be typed.
- `TYPESCRIPT_RUNTIME.md` — runtime discovery affects whether helpers generate bash or node scripts.

## Status

Not yet designed. This document captures the intent and tracks it as future work.
