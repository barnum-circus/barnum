# GSD - Get Shell Scripts Done

High-level JSON-based orchestrator that sits on top of `agent_pool`.

## Overview

GSD provides a declarative way to define task state machines via JSON config. It:
- Connects to an existing `agent_pool` daemon
- Wakes agents via a configurable script
- Validates tasks against JSON schemas at runtime
- Generates markdown documentation for agents
- Handles timeouts and task requeuing

## Config Format (`gsd.json`)

```json
{
  "options": {
    "timeout": 120,
    "max_retries": 3
  },
  "steps": [
    {
      "name": "Analyze",
      "schema": { "kind": "Inline", "value": { "type": "object", "properties": { "file": { "type": "string" } } } },
      "instructions": "Analyze the given file and determine what changes are needed.",
      "next": ["Implement", "Done"]
    },
    {
      "name": "Implement",
      "schema": { "kind": "Link", "path": "./schemas/implement.json" },
      "instructions": "Implement the changes described.",
      "next": ["Test", "Analyze"]
    },
    {
      "name": "Test",
      "schema": null,
      "instructions": "Run tests and verify the implementation.",
      "next": ["Done", "Implement"]
    },
    {
      "name": "Done",
      "schema": { "kind": "Inline", "value": { "type": "object", "properties": { "summary": { "type": "string" } } } },
      "instructions": "Task complete. Provide a summary.",
      "next": []
    }
  ]
}
```

### Config Fields

**`options`** (optional):
- `timeout`: Seconds before a task times out (default: no timeout)
- `max_retries`: Max times to requeue a timed-out task (default: 0)

**`steps`** (required): Array of step definitions:
- `name`: Step identifier (UpperCamelCase)
- `schema`: JSON Schema for the step's value payload
  - `null` → accepts any JSON value
  - `{ "kind": "Inline", "value": {...} }` → inline schema
  - `{ "kind": "Link", "path": "..." }` → path to schema file
- `instructions`: Markdown instructions shown to agents
- `next`: Array of valid next step names (empty = terminal step)

## Task Format

Tasks are JSON objects with `kind` and `value`:

```json
{"kind": "Analyze", "value": {"file": "src/main.rs"}}
```

Agent responses are arrays of tasks:

```json
[
  {"kind": "Implement", "value": {"changes": "..."}}
]
```

## CLI Usage

```bash
# Basic usage - pipe initial tasks
echo '[{"kind": "Analyze", "value": {"file": "main.rs"}}]' | gsd run config.json

# With wake script to notify agents
gsd run config.json --wake ./wake-agents.sh --initial '[{"kind": "Analyze", "value": {}}]'

# Connect to specific agent_pool
gsd run config.json --root /tmp/my-pool --initial tasks.json
```

### Commands

**`gsd run <config>`** - Run the state machine
- `--root <path>`: agent_pool root directory (default: temp dir)
- `--wake <script>`: Script to call to wake agents
- `--initial <json|file>`: Initial tasks (JSON string or path to file)

**`gsd schema <config>`** - Generate combined JSON schema

**`gsd docs <config>`** - Generate markdown documentation for agents

## Agent Documentation

GSD auto-generates markdown for agents describing valid actions:

```markdown
# Current Step: Analyze

Analyze the given file and determine what changes are needed.

## Valid Responses

You must return a JSON array of tasks. Valid next steps:

### Implement
```json
{"kind": "Implement", "value": <object matching schema>}
```

### Done
```json
{"kind": "Done", "value": <object matching schema>}
```
```

## Timeout Behavior

1. Task dispatched to agent with timeout info in instructions
2. If agent doesn't respond within `timeout` seconds:
   - GSD cancels the submit (kills the process)
   - agent_pool sees submit died, clears the pending response
   - Agent may continue running (output ignored)
   - Task requeued if `max_retries` not exceeded
3. Future: ability to signal agent to stop (TODO)

## Runtime Validation

All validation happens at runtime since tasks are opaque JSON:
1. Incoming task validated against step's schema
2. Agent response validated:
   - Must be JSON array
   - Each item must have `kind` matching a valid `next` step
   - Each item's `value` validated against target step's schema
3. Invalid responses: logged and discarded (task requeued)

## Architecture

```
┌─────────────┐     ┌─────────────┐     ┌─────────┐
│   gsd run   │────▶│ agent_pool  │────▶│ agents  │
│  (config)   │◀────│  (daemon)   │◀────│         │
└─────────────┘     └─────────────┘     └─────────┘
      │                                       ▲
      │         wake script                   │
      └───────────────────────────────────────┘
```

## File Structure

```
crates/gsd/
├── Cargo.toml
├── DESIGN.md
└── src/
    ├── main.rs       # CLI entry point
    ├── lib.rs        # Public API
    ├── config.rs     # Config parsing
    ├── schema.rs     # JSON schema handling and validation
    ├── docs.rs       # Markdown generation
    └── runner.rs     # State machine executor
```
