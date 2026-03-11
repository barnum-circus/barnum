# Fan-Out with Finally

Use `finally` to run a follow-up step after all parallel work completes.

## The Pattern

```
┌──────────────────────────────────────────────────────────┐
│  ListFiles (with finally)                                │
│                                                          │
│  ListFiles ──┬──→ Refactor(main.rs)                      │
│              ├──→ Refactor(lib.rs)                        │
│              └──→ Refactor(utils.rs)                      │
│                                                          │
│  ═══════════════════════════════════════════════════════  │
│  After ALL descendants complete:                         │
│                                                          │
│  finally ──→ Commit ──→ Done                             │
└──────────────────────────────────────────────────────────┘
```

## Example: Parallel Refactoring with Commit

List files, refactor them all in parallel, then commit the changes.

```jsonc
{
  "entrypoint": "ListFiles",
  "steps": [
    {
      "name": "ListFiles",
      "value_schema": {
        "type": "object",
        "required": ["directory"],
        "properties": {
          "directory": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        // Find all Rust files and emit one Refactor task per file.
        "script": "jq -r '.value.directory' | xargs -I{} find {} -name '*.rs' | jq -R -s 'split(\"\\n\") | map(select(length > 0)) | map({kind: \"Refactor\", value: {file: .}})'"
      },
      // After all Refactor tasks finish, transition to Commit.
      "finally": { "kind": "Command", "script": "echo '[{\"kind\": \"Commit\", \"value\": {\"message\": \"Apply refactors\"}}]'" },
      "next": ["Refactor"]
    },
    {
      "name": "Refactor",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Read the file at the path provided. Refactor it to improve readability and remove dead code. Write the changes back to disk. Return `[]`." }
      },
      "next": []
    },
    {
      "name": "Commit",
      "value_schema": {
        "type": "object",
        "required": ["message"],
        "properties": {
          "message": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        // Stage all changes and commit.
        "script": "MSG=$(jq -r '.value.message') && git add -A && git commit -m \"$MSG\" && echo '[]'"
      },
      "next": []
    }
  ]
}
```

## Running

```bash
barnum run --config config.json --pool agents --entrypoint-value '{"directory": "src"}'
```

## How It Works

1. **ListFiles** runs `find` to discover `.rs` files and fans out one `Refactor` task per file.
2. **Refactor** tasks run in parallel (up to `max_concurrency`). Each agent reads a file, makes changes, and writes it back.
3. **finally** fires after every `Refactor` descendant completes. It emits a single `Commit` task.
4. **Commit** stages and commits all the changes.

## Key Points

- `finally` runs after ALL descendants complete (not just direct children)
- `finally` receives the original task's value on stdin
- `finally` outputs a JSON array of next tasks to spawn follow-up work
- The finally hook here is an inline script — no external file needed
- The pattern enables: fan-out → parallel work → single follow-up action
