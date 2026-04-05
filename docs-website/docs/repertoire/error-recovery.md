---
image: /img/og/repertoire-error-recovery.png
---

# Error Recovery

Use command steps to catch failures and route them to recovery steps instead of dropping tasks.

## Why This Pattern?

By default, failed tasks are retried and eventually dropped. But some failures are recoverable. A compilation error after a refactor can be fixed, a timeout on a flaky API can be retried with different parameters. Command steps can verify outcomes and convert failures into new tasks.

## The Pattern

```
DoWork → CheckResult → FixError → DoWork
                    ↘ Done
```

## Example: Self-Healing Refactor

An agent refactors a file. A command step checks the build. If it breaks, a recovery agent attempts to fix it.

```jsonc
{
  "entrypoint": "Refactor",
  "steps": [
    {
      "name": "Refactor",
      "value_schema": {
        "type": "object",
        "required": ["file", "task"],
        "properties": {
          "file": { "type": "string" },
          "task": { "type": "string" },
          "previous_error": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "Refactor the file as described in `task`. If `previous_error` is present, a prior attempt broke the build — use the error to guide your approach.\n\nReturn `[]` when done." }
      },
      "next": ["CheckBuild"]
    },
    {
      "name": "CheckBuild",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": { "type": "string" },
          "task": { "type": "string" }
        }
      },
      "action": {
        "kind": "Command",
        // Run cargo check. If it passes, return []. If it fails, spawn a FixBuild task.
        "script": "INPUT=$(cat) && FILE=$(echo \"$INPUT\" | jq -r '.value.file') && if cargo check 2>/tmp/build_err.txt; then echo '[]'; else ERROR=$(cat /tmp/build_err.txt) && echo \"[{\\\"kind\\\": \\\"FixBuild\\\", \\\"value\\\": {\\\"file\\\": \\\"$FILE\\\", \\\"error\\\": $(echo \"$ERROR\" | jq -Rs .)}}]\"; fi"
      },
      "next": ["FixBuild"]
    },
    {
      "name": "FixBuild",
      "value_schema": {
        "type": "object",
        "required": ["file", "error"],
        "properties": {
          "file": { "type": "string" },
          "error": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "The build broke after a refactor. You receive the file that was changed and the build error.\n\nFix the build error. Focus only on making the build pass — don't change the intent of the refactor.\n\nReturn `[]` when done." }
      },
      "next": []
    }
  ]
}
```

## How It Works

1. **Refactor** agent modifies the file as requested, spawns CheckBuild.
2. **CheckBuild** command step runs `cargo check` to verify the build.
3. If the build passes, returns `[]`. Done.
4. If the build fails, returns a **FixBuild** task containing the error output.
5. **FixBuild** agent reads the error and fixes the build.

## Resource Cleanup with Finally

Use `finally` to clean up resources after all children complete:

```jsonc
{
  "entrypoint": "BatchProcess",
  "steps": [
    {
      "name": "BatchProcess",
      "value_schema": {
        "type": "object",
        "required": ["files"],
        "properties": {
          "files": { "type": "array", "items": { "type": "string" } }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "Fan out: return one ProcessFile task per file.\n\n```json\n[{\"kind\": \"ProcessFile\", \"value\": {\"file\": \"src/main.rs\"}}]\n```" }
      },
      "next": ["ProcessFile"],
      // Finally cleans up after ALL files are processed.
      "finally": { "kind": "Command", "script": "echo '[]'" }
    },
    {
      "name": "ProcessFile",
      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "kind": "Inline", "value": "Process this file.\n\nReturn `[]` when done." }
      },
      "next": []
    }
  ]
}
```

## Key Points

- Command steps can check outcomes and route failures to recovery steps
- `finally` hooks clean up after all descendants complete (not just direct children)
- Recovery steps can loop back to the original step for retry-after-fix patterns
