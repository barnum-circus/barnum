# Pluggable Action Kinds

## Motivation

Today, non-agent work in Barnum is done via `Command` actions — opaque bash strings that receive JSON on stdin and emit JSON on stdout. This works, but it has problems:

1. **No structure.** A git commit is expressed as `"script": "jq -r '.value.message' | xargs git commit -m"`. There's no validation that the message field exists, no type safety, and the bash string is fragile.

2. **No reuse.** Every workflow that needs git operations reinvents the same bash incantations. There's no way to share or standardize common operations.

3. **No discoverability.** Users can't browse available operations or get editor completion for what a `Git` action accepts.

4. **No validation.** The bash script can do anything. A typo in a jq expression silently produces empty output. There's no way to validate the action's parameters at config-parse time.

The idea: make the action kind extensible. Instead of just `Pool` and `Command`, allow pluggable action kinds like `Git`, `Http`, `FileSystem`, `TypeScript`, etc. Each kind defines a parameter schema, and Barnum validates the parameters at config load time.

## Current State

### Action enum (closed)

`crates/barnum_config/src/config.rs:161-187`:
```rust
#[serde(tag = "kind")]
pub enum ActionFile {
    Pool { instructions: MaybeLinked<Instructions> },
    Command { script: String },
}
```

This is a closed enum — adding a new action kind requires modifying the Rust source code, recompiling, and releasing.

### Dispatch (hardcoded match)

`crates/barnum_config/src/runner/mod.rs` (in `TaskRunner::dispatch()`):
```rust
match &step.action {
    Action::Pool { .. } => { /* submit to troupe pool */ }
    Action::Command { script } => { /* run shell command */ }
}
```

Adding a new action kind requires adding a new match arm here, plus a new dispatch function, a new `SubmitResult` variant, and new response processing logic.

## Proposed Design

### Core concept: Action kinds as plugins

An action kind is defined by:
1. **A name** (e.g., `"Git"`, `"Http"`, `"Transform"`)
2. **A parameter schema** (JSON Schema that validates the action's config)
3. **An executor** (something that takes the task + parameters and returns follow-up tasks)

### What this looks like in config

**Before (opaque Command):**
```jsonc
{
  "name": "CommitChanges",
  "action": {
    "kind": "Command",
    "script": "MSG=$(jq -r '.value.message'); git add -A && git commit -m \"$MSG\""
  },
  "next": ["Push"]
}
```

**After (structured Git action):**
```jsonc
{
  "name": "CommitChanges",
  "action": {
    "kind": "Git",
    "operation": "commit",
    "params": {
      "message": "{{value.message}}",
      "add_all": true
    }
  },
  "next": ["Push"]
}
```

Or even more declaratively:
```jsonc
{
  "name": "CommitChanges",
  "action": {
    "kind": "Git",
    "commit": {
      "message": "{{value.message}}",
      "paths": ["{{value.file}}"]
    }
  },
  "next": ["Push"]
}
```

### How action kinds get registered

Three possible registration mechanisms, from simple to complex:

#### Option 1: Built-in action kinds (curated set)

Barnum ships with a fixed set of action kinds beyond `Pool` and `Command`. These are compiled into the binary:

- `Pool` — send to AI agents (exists)
- `Command` — run bash (exists)
- `Git` — git operations (commit, push, branch, etc.)
- `Http` — HTTP requests
- `FileSystem` — read/write/copy files
- `Transform` — JSON transformations (jq-like but structured)
- `Typescript` — run a TypeScript function (see TYPESCRIPT_API.md)

**Pros:** Simple. No plugin system needed. Schema validation is built in.
**Cons:** Barnum must add and maintain every action kind. Can't extend without recompiling.

#### Option 2: External action kind definitions (JSON Schema + Command executor)

Users define custom action kinds in a `kinds/` directory or a `"kinds"` section of the config. Each kind has a JSON Schema for its parameters and a command that executes it:

```jsonc
{
  "kinds": {
    "Git": {
      "schema": {"link": "kinds/git-schema.json"},
      "executor": "node kinds/git-executor.js"
    },
    "Slack": {
      "schema": {"link": "kinds/slack-schema.json"},
      "executor": "node kinds/slack-executor.js"
    }
  },
  "steps": [...]
}
```

The executor receives the task + validated parameters on stdin and returns follow-up tasks on stdout — same contract as `Command`, but with schema-validated parameters.

**Pros:** Extensible without recompiling Barnum. Users can create and share action kinds.
**Cons:** Executor is still a subprocess with stdin/stdout JSON. Adds config complexity.

#### Option 3: Plugin registry (npm packages)

Action kinds are npm packages that export a schema and an executor:

```jsonc
{
  "plugins": ["@barnum/plugin-git", "@barnum/plugin-slack"],
  "steps": [...]
}
```

Each plugin package contains:
- `schema.json` — parameter schema for the action kind
- `executor.js` — the execution logic
- Optionally, TypeScript types for the builder API

**Pros:** Ecosystem play. Share and discover action kinds via npm.
**Cons:** Complex. Requires plugin loading, version management, security considerations.

### Recommended approach: Option 1 first, Option 2 as escape hatch

Start with a curated set of built-in action kinds (Option 1). Add the external-definition mechanism (Option 2) as the escape hatch for custom kinds. Skip Option 3 until there's actual demand for a plugin ecosystem.

## How a built-in action kind works

Taking `Git` as a concrete example:

### Parameter schema

```json
{
  "type": "object",
  "oneOf": [
    {
      "properties": {
        "commit": {
          "type": "object",
          "properties": {
            "message": {"type": "string"},
            "paths": {"type": "array", "items": {"type": "string"}},
            "add_all": {"type": "boolean", "default": false}
          },
          "required": ["message"]
        }
      },
      "required": ["commit"]
    },
    {
      "properties": {
        "push": {
          "type": "object",
          "properties": {
            "remote": {"type": "string", "default": "origin"},
            "branch": {"type": "string"}
          }
        }
      },
      "required": ["push"]
    }
  ]
}
```

### Execution

When Barnum encounters `{"kind": "Git", "commit": {"message": "...", "paths": [...]}}`:

1. Validate parameters against the Git schema at config load time
2. At runtime, resolve any template expressions (like `{{value.message}}`) against the task value
3. Execute the git operation directly (no shell, no subprocess — just `std::process::Command` with the right args)
4. Return follow-up tasks based on the result

### Template expressions

The `{{value.message}}` syntax lets parameters reference the task's input value. This is resolved at runtime, not at config parse time. It's essentially string interpolation from the task payload into the action parameters.

Alternative: skip templates entirely. The task value IS the parameters. If the step's `value_schema` matches what Git needs, the action kind reads directly from the value:

```jsonc
{
  "name": "CommitChanges",
  "value_schema": {
    "type": "object",
    "properties": {
      "message": {"type": "string"},
      "paths": {"type": "array", "items": {"type": "string"}}
    },
    "required": ["message"]
  },
  "action": {
    "kind": "Git",
    "operation": "commit"
  },
  "next": ["Push"]
}
```

Here the action kind knows that for `"operation": "commit"`, it reads `message` and `paths` from the task value. The `value_schema` provides the validation. No templates needed.

This is cleaner but constrains the task value to match the action's expectations exactly. The pre-hook can bridge the gap if the incoming shape differs.

## Rust implementation sketch

### Open the action enum

```rust
// config.rs
#[serde(tag = "kind")]
pub enum ActionFile {
    Pool { instructions: MaybeLinked<Instructions> },
    Command { script: String },
    // New built-in kinds:
    Git { operation: String, #[serde(flatten)] params: serde_json::Value },
    // Or more structured:
    Git(GitAction),
    // ...
    // Escape hatch for user-defined kinds:
    Custom { kind_name: String, executor: String, params: serde_json::Value },
}
```

### Or: keep the enum closed, use serde's content tag

```rust
#[serde(tag = "kind", content = "params")]
pub enum ActionFile {
    Pool { instructions: MaybeLinked<Instructions> },
    Command { script: String },
    Git(GitParams),
    Http(HttpParams),
    // ...
}
```

### Or: hybrid approach

```rust
// Built-in kinds are enum variants. Unknown kinds fall through to Custom.
#[serde(tag = "kind")]
pub enum ActionFile {
    Pool { ... },
    Command { ... },
    Git { ... },
    #[serde(other)]
    Custom,  // Catch-all for user-defined kinds
}
```

The `#[serde(other)]` approach is clean for deserialization but loses the parameters. A custom deserializer could capture them.

## Relationship to TypeScript API

The TypeScript API (see `TYPESCRIPT_API.md`) and pluggable action kinds are complementary:

- **TypeScript API** = write step handlers in TypeScript with type safety
- **Pluggable action kinds** = built-in structured commands with schema validation

They intersect at: "TypeScript" could be a pluggable action kind, and the TypeScript builder API could expose all action kinds as typed methods.

```typescript
// Builder API with pluggable kinds
const commit = step("CommitChanges")
  .value<{ message: string; paths: string[] }>()
  .git({ operation: "commit" })  // typed Git action
  .next("Push");
```

## Open Questions

1. **How many built-in kinds?** Starting with Git makes sense given the codebase's focus on code-related workflows. But where do we draw the line? Git, Http, FileSystem feels reasonable. Slack, Email, Database feels like scope creep.

2. **Template expressions or direct value mapping?** Templates (`{{value.field}}`) are flexible but add a new expression language. Direct value mapping is simpler but requires the task value to match the action's expected shape exactly.

3. **Should built-in kinds be implemented in Rust or as bundled executors?** Rust is faster and has no runtime dependency, but every new kind requires a Rust change and recompile. Bundled JS/TS executors are slower but can be updated independently.

4. **How does this interact with the JSON Schema generation?** Today `barnum-config-schema.json` is generated from the Rust types via `schemars`. Adding a `Git` variant to `ActionFile` automatically adds it to the schema. But user-defined kinds (Option 2) can't be in the schema at compile time.

5. **Do we need parameter validation at config load time, or is runtime validation sufficient?** Config-time validation catches errors early (before any work starts). Runtime validation is simpler to implement. The `value_schema` system already does runtime validation for task values.
