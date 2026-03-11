# Sequential Processing

Process items one at a time by having a step loop back to itself. This is particularly useful when **multiple changes need to happen to the same file** in a specific order — each step sees the result of the previous one.

> **Note:** The self-looping pattern works but is a bit awkward. Future versions of Barnum will include better primitives for sequential workflows.

## The Pattern

```
ProcessNext ──→ ProcessNext ──→ ProcessNext ──→ Done
  (item 1)        (item 2)        (item 3)
```

Instead of fanning out all items in parallel, the step processes the first item, then returns a task for the same step with the remaining items. This guarantees items are handled one at a time, in order.

## Example: Sequential File Migration

Migrate database schema files one at a time, in order, so each migration builds on the previous one.

```jsonc
{
  "entrypoint": "ProcessNext",
  "options": {
    "max_concurrency": 1
  },
  "steps": [
    {
      "name": "ProcessNext",
      "value_schema": {
        "type": "object",
        "required": ["remaining"],
        "properties": {
          "remaining": {
            "type": "array",
            "items": { "type": "string" }
          },
          "completed": {
            "type": "array",
            "items": { "type": "string" }
          }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "You receive a list of migration files in `remaining`. Apply ONLY the first migration file. Read the SQL file, execute the migration, and verify it succeeded.\n\nIf there are more files after the first, return:\n```json\n[{\"kind\": \"ProcessNext\", \"value\": {\"remaining\": [\"002.sql\", \"003.sql\"], \"completed\": [\"001.sql\"]}}]\n```\n\nIf this was the last file, return `[]`." }
      },
      // The step can transition to itself to continue the chain.
      "next": ["ProcessNext"]
    }
  ]
}
```

## Running

```bash
barnum run --config config.json \
  --entrypoint-value '{"remaining": ["001-create-users.sql", "002-add-email.sql", "003-add-index.sql"], "completed": []}'
```

## How It Works

1. **ProcessNext** receives the full list: `["001.sql", "002.sql", "003.sql"]`
2. The agent applies `001.sql`, then returns a task for **ProcessNext** with `remaining: ["002.sql", "003.sql"]`
3. That task applies `002.sql`, returns **ProcessNext** with `remaining: ["003.sql"]`
4. That task applies `003.sql`, returns `[]` (done)

Each step runs to completion before the next one starts, enforcing strict ordering.

## Command Variant

If the processing is deterministic, use a Command action instead:

```jsonc
{
  "entrypoint": "ProcessNext",
  "steps": [
    {
      "name": "ProcessNext",
      "value_schema": {
        "type": "object",
        "required": ["remaining"],
        "properties": {
          "remaining": { "type": "array", "items": { "type": "string" } }
        }
      },
      "action": {
        "kind": "Command",
        // Apply the first migration, then return ProcessNext with the rest (or [] if done).
        "script": "INPUT=$(cat) && FILE=$(echo \"$INPUT\" | jq -r '.value.remaining[0]') && psql -f \"$FILE\" && echo \"$INPUT\" | jq 'if (.value.remaining | length) > 1 then [{kind: \"ProcessNext\", value: {remaining: .value.remaining[1:]}}] else [] end'"
      },
      "next": ["ProcessNext"]
    }
  ]
}
```

## Key Points

- A step can list itself in `next` to create a self-loop
- The agent/command peels off one item and returns the rest
- Return `[]` when the list is empty to terminate
- `max_concurrency: 1` is a useful safety net but not strictly required — the sequential structure itself ensures ordering since each task only emits one successor
