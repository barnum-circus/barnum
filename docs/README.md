# Barnum Configuration

Example configuration with annotations. Barnum config files are JSON;
the comments below (JSONC) are for explanation only.

```jsonc
{
  // Optional: JSON Schema reference for editor autocompletion and validation.
  // Ignored at runtime. Generate with `barnum config schema`.
  "$schema": "./barnum.schema.json",

  // Entry point step name. When set, the workflow starts here and you can
  // pass the initial value with --entrypoint-value (defaults to {}).
  // This is the first step that runs.
  "entrypoint": "Coordinate",

  // Global runtime options. Every step inherits these unless overridden.
  "options": {
    // Timeout in seconds per task (null = no timeout).
    "timeout": 120,
    // Maximum retry attempts per task.
    "max_retries": 2,
    // Retry when an agent returns a response that fails validation.
    "retry_on_invalid_response": true,
    // Retry when a task exceeds its timeout.
    "retry_on_timeout": true,
    // Maximum tasks executing concurrently (null = unlimited).
    "max_concurrency": 5
  },

  "steps": [
    {
      "name": "Coordinate",

      // Value schema: an optional JSON Schema that validates the data received
      // by the action (the agent or command). Use it to ensure the payload has
      // a valid shape before the action ever sees it. Can be inline (as below)
      // or a reference to an external file: {"link": "./schemas/coordinate.json"}.
      "value_schema": {
        "type": "object",
        "required": ["files"],
        "properties": {
          "files": {
            "type": "array",
            "items": { "type": "string" }
          }
        }
      },

      // Action kind "Command" runs a local shell script.
      // stdin receives the task JSON, stdout must produce a JSON array of
      // next tasks. Exit 0 = success, non-zero = error (triggers retry).
      "action": {
        "kind": "Command",
        // Fan out: emit one AnalyzeFile task per file in the input array.
        "script": "jq -c '.value.files[] | {kind: \"AnalyzeFile\", value: {file: .}}' | jq -s"
      },

      // Valid next steps this action can transition to.
      "next": ["AnalyzeFile"],

      // Finally hook: runs after ALL descendants of this task complete
      // (not just direct children). Receives the original task's value on
      // stdin and outputs a JSON array of follow-up tasks on stdout.
      "finally": { "kind": "Command", "script": "echo '[]'" }
    },
    {
      "name": "AnalyzeFile",

      "value_schema": {
        "type": "object",
        "required": ["file"],
        "properties": {
          "file": { "type": "string" }
        }
      },

      // Pre hook: runs before the action. Receives the task value on stdin,
      // outputs the (possibly transformed) value on stdout. Use it to enrich
      // context, read files, or validate input.
      "pre": { "kind": "Command", "script": "jq '. + {timestamp: now | todate}'" },

      // Action kind "Pool" sends the task to an agent pool for processing.
      // Instructions are markdown shown to the agent.
      "action": {
        "kind": "Pool",
        "instructions": {
          // Instructions can also be a file reference: {"link": "./prompts/analyze.md"}
          "inline": "Analyze this file for issues. Return `[]`."
        }
      },

      // Post hook: runs after the action (even on timeout or error). Receives
      // a result JSON on stdin with kind "Success", "Timeout", "Error", or
      // "PreHookError". Can modify the `next` tasks in the result. Outputs
      // the (possibly modified) result on stdout.
      "post": { "kind": "Command", "script": "cat" },

      // Per-step options override the global defaults.
      "options": {
        "timeout": 300,
        "max_retries": 3
      },

      // Empty next = terminal step (no further transitions).
      "next": []
    }
  ]
}
```

## Running

```bash
# With entrypoint (config has "entrypoint" set):
barnum run --config config.json
barnum run --config config.json --entrypoint-value '{"files": ["src/main.rs"]}'
```

## Writing command scripts

Commands receive the full task JSON on stdin. Use `jq` to extract
parameters from the task value.

### Stdin format

The JSON piped to stdin looks like this:

```jsonc
{
  "kind": "StepName",
  "value": {
    // Whatever the upstream step or entrypoint-value provided.
    "file": "src/main.rs",
    "options": { "verbose": true }
  }
}
```

### Extracting parameters with jq

```bash
#!/bin/bash
set -e

# Read once, extract fields
INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.value.file')
VERBOSE=$(echo "$INPUT" | jq -r '.value.options.verbose')
```

### Inline scripts

For simple transformations, the script can be an inline jq pipeline
instead of a file reference:

```jsonc
{
  "name": "Split",
  "action": {
    "kind": "Command",
    // Fan out: take an array of items and emit one task per item.
    // jq -c produces compact output; `| jq -s` collects into an array.
    "script": "jq -c '.value.items[] | {kind: \"Process\", value: .}' | jq -s"
  },
  "next": ["Process"]
}
```

### Stdout format

Commands must output a JSON array of next tasks on stdout:

```jsonc
[
  // Each element is a task to enqueue.
  { "kind": "NextStep", "value": { "result": "data" } },
  { "kind": "NextStep", "value": { "result": "more data" } }
]
```

Return `[]` to end the chain (terminal).

### Common patterns

**Read a file referenced in the task value:**

```bash
#!/bin/bash
set -e
INPUT=$(cat)
FILE=$(echo "$INPUT" | jq -r '.value.file')
CONTENTS=$(cat "$FILE")
echo "[{\"kind\": \"Process\", \"value\": {\"contents\": $(echo "$CONTENTS" | jq -Rs)}}]"
```

**Fan out over an array:**

```jsonc
{
  "action": {
    "kind": "Command",
    "script": "jq -c '.value.items[] | {kind: \"Process\", value: .}' | jq -s"
  }
}
```

**Call an API:**

```jsonc
{
  "action": {
    "kind": "Command",
    "script": "jq -r '.value.url' | xargs curl -s | jq '[{kind: \"Parse\", value: .}]'"
  }
}
```

### Exit codes

- **exit 0**: success, stdout is parsed as the response
- **exit non-zero**: error, triggers the retry policy

## Useful commands

```bash
barnum config schema              # Print JSON schema for config files
barnum config validate config.json # Validate a config file
barnum config docs config.json    # Generate markdown docs from config
barnum config graph config.json   # Generate DOT graph for GraphViz
```
