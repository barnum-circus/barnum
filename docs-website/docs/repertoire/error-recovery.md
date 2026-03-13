# Error Recovery

Use post hooks to catch failures and route them to recovery steps instead of dropping tasks.

## Why This Pattern?

By default, failed tasks are retried and eventually dropped. But some failures are recoverable. A compilation error after a refactor can be fixed, a timeout on a flaky API can be retried with different parameters. Post hooks see **every** outcome (success, timeout, error) and can convert failures into new tasks.

## The Pattern

```
                   ┌──── Success ──→ Done
                   │
DoWork ─→ [post] ──┤
                   │
                   └──── Error ──→ FixError ──→ DoWork
```

## Example: Self-Healing Refactor

An agent refactors a file. If the build breaks, a recovery agent attempts to fix it.

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
        "instructions": { "inline": "Refactor the file as described in `task`. If `previous_error` is present, a prior attempt broke the build — use the error to guide your approach.\n\nReturn `[]` when done." }
      },
      // Post hook checks if the build still passes.
      "post": { "kind": "Command", "script": "INPUT=$(cat) && KIND=$(echo \"$INPUT\" | jq -r '.kind') && if [ \"$KIND\" != \"Success\" ]; then echo \"$INPUT\"; exit 0; fi && FILE=$(echo \"$INPUT\" | jq -r '.input.file') && if cargo check 2>/tmp/build_err.txt; then echo \"$INPUT\"; else ERROR=$(cat /tmp/build_err.txt) && echo \"$INPUT\" | jq --arg err \"$ERROR\" --arg file \"$FILE\" '.next = [{kind: \"FixBuild\", value: {file: $file, error: $err}}]'; fi" },
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
        "instructions": { "inline": "The build broke after a refactor. You receive the file that was changed and the build error.\n\nFix the build error. Focus only on making the build pass — don't change the intent of the refactor.\n\nReturn `[]` when done." }
      },
      "next": []
    }
  ]
}
```

## Running

```bash
barnum run config.json \
  --entrypoint-value '{"file": "src/lib.rs", "task": "Extract the Config struct into its own module"}'
```

## How It Works

1. **Refactor** agent modifies the file as requested.
2. The **post hook** runs `cargo check` to verify the build.
3. If the build passes, the result flows through unchanged. Task is done.
4. If the build fails, the post hook replaces `next` with a **FixBuild** task containing the error output.
5. **FixBuild** agent reads the error and fixes the build.

## Resource Cleanup

Post hooks are also useful for cleaning up resources. Here's a pattern using a temp directory:

```jsonc
{
  "entrypoint": "Process",
  "steps": [
    {
      "name": "Process",
      "value_schema": {
        "type": "object",
        "required": ["url"],
        "properties": {
          "url": { "type": "string" }
        }
      },
      // Pre hook creates a temp directory and adds it to the value.
      "pre": { "kind": "Command", "script": "INPUT=$(cat) && TMPDIR=$(mktemp -d) && echo \"$INPUT\" | jq --arg dir \"$TMPDIR\" '. + {tmpdir: $dir}'" },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Download and process the file at `url`. Use the `tmpdir` directory for any intermediate files.\n\nReturn `[]` when done." }
      },
      // Post hook cleans up the temp directory regardless of outcome.
      "post": { "kind": "Command", "script": "INPUT=$(cat) && TMPDIR=$(echo \"$INPUT\" | jq -r '.input.tmpdir // empty') && [ -n \"$TMPDIR\" ] && rm -rf \"$TMPDIR\"; echo \"$INPUT\"" },
      "next": []
    }
  ]
}
```

The pre hook creates a temp directory and injects it into the value. The post hook cleans it up, even if the action timed out or errored.

## Finally-Based Cleanup

For fan-out workflows, use `finally` to clean up after all children complete:

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
          "files": { "type": "array", "items": { "type": "string" } },
          "workdir": { "type": "string" }
        }
      },
      // Pre hook creates a shared workspace.
      "pre": { "kind": "Command", "script": "INPUT=$(cat) && WORKDIR=$(mktemp -d) && echo \"$INPUT\" | jq --arg dir \"$WORKDIR\" '. + {workdir: $dir}'" },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Fan out: return one ProcessFile task per file, passing the workdir to each.\n\n```json\n[{\"kind\": \"ProcessFile\", \"value\": {\"file\": \"src/main.rs\", \"workdir\": \"/tmp/abc123\"}}]\n```" }
      },
      "next": ["ProcessFile"],
      // Finally cleans up the shared workspace after ALL files are processed.
      "finally": { "kind": "Command", "script": "INPUT=$(cat) && WORKDIR=$(echo \"$INPUT\" | jq -r '.workdir // empty') && [ -n \"$WORKDIR\" ] && rm -rf \"$WORKDIR\"; echo '[]'" }
    },
    {
      "name": "ProcessFile",
      "value_schema": {
        "type": "object",
        "required": ["file", "workdir"],
        "properties": {
          "file": { "type": "string" },
          "workdir": { "type": "string" }
        }
      },
      "action": {
        "kind": "Pool",
        "instructions": { "inline": "Process this file. Use `workdir` for intermediate output.\n\nReturn `[]` when done." }
      },
      "next": []
    }
  ]
}
```

## Key Points

- Post hooks run on **every** outcome: Success, Timeout, Error, PreHookError
- Post hooks can replace the `next` array to route failures to recovery steps
- Pre hooks can create resources; post hooks can clean them up
- `finally` hooks clean up after all descendants complete (not just direct children)
- Recovery steps can loop back to the original step (add it to `next`) for retry-after-fix patterns
