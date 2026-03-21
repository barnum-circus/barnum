# Claude CLI Action Kind

## Motivation

Today, using an LLM in a Barnum workflow requires the `Pool` action kind: tasks go through the troupe agent pool, where a long-lived agent process picks them up, sends them to an LLM, and returns the result. This works well for persistent agents with state, but it's heavy machinery for what is often a stateless function call: "given this prompt and this data, return structured JSON."

A `Claude` action kind would invoke Claude CLI directly — no pool, no daemon, no agent process. Each task spawns a `claude` subprocess with the prompt and data, gets the response, and continues. This is simpler, more portable (no troupe daemon needed), and more natural for workflows where each step is an independent LLM call.

This builds on the pluggable action kinds design (see `PLUGGABLE_ACTION_KINDS.md`). The `Claude` kind is the first concrete example of a pluggable action kind that isn't `Pool` or `Command`.

## What it looks like

```jsonc
{
  "steps": [
    {
      "name": "Analyze",
      "action": {
        "kind": "Claude",
        "prompt": {"link": "prompts/analyze.md"},
        "model": "sonnet",
        "max_tokens": 4096,
        "output_dir": "./outputs"
      },
      "next": ["Implement"]
    },
    {
      "name": "Implement",
      "action": {
        "kind": "Claude",
        "prompt": {"inline": "Implement the changes described in the analysis."},
        "model": "sonnet",
        "allowed_tools": ["Read", "Edit", "Write", "Bash"]
      },
      "next": []
    }
  ]
}
```

Key properties:
- `prompt` uses the same `MaybeLinked` pattern as Pool's `instructions` (inline or file link)
- `model` selects the Claude model
- `output_dir` — directory where the full conversation transcript is written (one file per task invocation)
- `allowed_tools` — which Claude Code tools to enable (for agentic use)

## How it works

### Invocation

Each task dispatched as a `Claude` action spawns a `claude` CLI subprocess:

```bash
claude --model sonnet --output-format json --max-tokens 4096 \
  --prompt "$(cat prompt.md)" \
  --stdin < task_value.json
```

Or using the `-p` (print, non-interactive) flag:

```bash
echo '{"kind": "Analyze", "value": {"file": "src/main.rs"}}' | \
  claude -p --model sonnet "$(cat prompt.md)"
```

The exact invocation depends on Claude CLI's API. The key point: it's a subprocess, same as `Command`, but structured rather than an opaque bash string.

### Response parsing

Claude CLI outputs text (or JSON if `--output-format json`). The `Claude` action kind parses the response the same way `Pool` does: expects a JSON array of `[{"kind": "NextStep", "value": {...}}]` follow-up tasks.

The prompt template should instruct Claude to return this format, just as Pool instructions do today. The difference is mechanical (subprocess vs agent pool), not semantic.

### Output capture

Every Claude invocation writes its full conversation transcript to `output_dir`:

```
outputs/
  task-0001-Analyze.json       # Full request + response
  task-0002-Implement.json     # Full request + response
  ...
```

Each file contains:
```json
{
  "task_id": 1,
  "step": "Analyze",
  "model": "sonnet",
  "input": {"file": "src/main.rs"},
  "prompt": "Analyze the file...",
  "response": "The file contains...",
  "parsed_output": [{"kind": "Implement", "value": {"plan": "..."}}],
  "usage": {"input_tokens": 1234, "output_tokens": 567},
  "duration_ms": 3200,
  "timestamp": "2026-03-21T17:30:00Z"
}
```

This is the raw material for visualization (see section below). Barnum doesn't visualize it — it just writes it.

## Relationship to Pool

| Aspect | Pool | Claude |
|--------|------|--------|
| **Runtime** | Troupe daemon + agent process | Direct subprocess |
| **State** | Agent has conversation history | Stateless per invocation |
| **Setup** | Requires `troupe start`, agent script | Just needs `claude` on PATH |
| **Concurrency** | Pool manages agent lifecycle | Barnum spawns N subprocesses |
| **Best for** | Multi-turn conversations, persistent context | Single-turn structured calls |

`Claude` doesn't replace `Pool`. Pool is better when you need agents with persistent state across turns, or when you want troupe's agent management. `Claude` is better for stateless fan-out: "analyze each of these 50 files independently."

## Config schema impact

This is where the pluggable action kinds design intersects with the generated `barnum-config-schema.json`.

### Current state

The schema is generated from the Rust `ActionFile` enum via `schemars`:

```rust
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum ActionFile {
    Pool { instructions: MaybeLinked<Instructions> },
    Command { script: String },
}
```

`cargo run -p barnum_config --bin build_barnum_schema` generates the schema, CI verifies it matches, and editors use it for validation/completion.

### Adding Claude as a built-in kind

Adding a `Claude` variant to `ActionFile` automatically extends the schema:

```rust
#[serde(tag = "kind")]
pub enum ActionFile {
    Pool { instructions: MaybeLinked<Instructions> },
    Command { script: String },
    Claude {
        prompt: MaybeLinked<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        max_tokens: Option<u32>,
        #[serde(default)]
        output_dir: Option<String>,
        #[serde(default)]
        allowed_tools: Option<Vec<String>>,
    },
}
```

The schema generator picks this up automatically. `barnum-config-schema.json` gets a third `oneOf` variant with all the Claude-specific fields documented. Editors show completion for `"kind": "Claude"` and validate the parameters.

### For user-defined action kinds

User-defined kinds (the "escape hatch" from `PLUGGABLE_ACTION_KINDS.md`) can't be in the compile-time schema. Two options:

1. **`additionalProperties: true` on ActionFile** — allow unknown kinds, lose validation. Bad.

2. **Composite schema** — the generated schema covers built-in kinds. A separate user-provided schema (or a `"kinds"` section in the config) defines custom kinds. The config validator merges them at runtime. Editors would need to reference both schemas.

3. **Schema generation includes registered kinds** — if the config has a `"kinds"` section declaring custom kinds with their parameter schemas, the `build_barnum_schema` tool could read the config and generate a complete schema. But this creates a circular dependency (schema depends on config, config references schema).

Recommendation: built-in kinds (Pool, Command, Claude) live in the Rust enum and get automatic schema generation. Custom kinds declared via `"kinds"` in config are validated at runtime only. This is acceptable because custom kinds are the escape hatch — power users who define custom kinds can live without editor completion. The common case (Pool, Command, Claude) gets full editor support.

## Implementation sketch

### New Rust types

```rust
// config.rs
#[serde(tag = "kind")]
pub enum ActionFile {
    Pool { instructions: MaybeLinked<Instructions> },
    Command { script: String },
    Claude {
        prompt: MaybeLinked<String>,
        #[serde(default)]
        model: Option<String>,
        #[serde(default)]
        max_tokens: Option<u32>,
        #[serde(default)]
        output_dir: Option<PathBuf>,
        #[serde(default)]
        allowed_tools: Option<Vec<String>>,
    },
}

// resolved.rs
pub enum Action {
    Pool { instructions: String },
    Command { script: String },
    Claude {
        prompt: String,             // resolved from MaybeLinked
        model: Option<String>,
        max_tokens: Option<u32>,
        output_dir: Option<PathBuf>,
        allowed_tools: Option<Vec<String>>,
    },
}
```

### New dispatch function

```rust
// dispatch.rs
pub fn dispatch_claude_task(
    ctx: TaskContext,
    prompt: &str,
    model: Option<&str>,
    max_tokens: Option<u32>,
    output_dir: Option<&Path>,
    allowed_tools: Option<&[String]>,
    working_dir: &Path,
    tx: &mpsc::Sender<InFlightResult>,
) {
    // 1. Run pre-hook if present
    // 2. Build prompt with task value interpolated
    // 3. Spawn `claude` subprocess
    // 4. Capture stdout as response
    // 5. Write transcript to output_dir
    // 6. Parse response as JSON task array
    // 7. Send result on tx
}
```

### New SubmitResult variant

```rust
pub enum SubmitResult {
    Pool { value: StepInputValue, response: io::Result<Response> },
    Command { value: StepInputValue, output: io::Result<String> },
    Claude { value: StepInputValue, output: io::Result<String> },
    Finally { value: StepInputValue, output: Result<String, String> },
    PreHookError(String),
}
```

`Claude` uses the same `output: io::Result<String>` as `Command` since both are subprocess stdout. The response processing path can share code.

### CLI detection

Similar to how barnum detects troupe via `cli_invoker`, we need to detect `claude`:

1. `CLAUDE_BIN` env var
2. `claude` on PATH
3. `npx @anthropic-ai/claude-code` as fallback

This could be another `InvokableCli` implementation, or simpler: just `which claude` since Claude CLI doesn't need the version-pinning complexity that troupe does.

## Open questions

1. **What is the exact Claude CLI invocation for structured output?** Need to verify `claude -p --output-format json` or equivalent. The prompt needs to instruct Claude to return `[{"kind": "...", "value": {...}}]` format.

2. **Should `Claude` action kind support multi-turn (agentic) mode?** Claude Code can run in agentic mode with tools. This is powerful but long-running. The timeout system handles this, but it changes the mental model from "quick structured call" to "autonomous agent session."

3. **How does `output_dir` interact with `state_log_path`?** The state log captures task submission/completion events (structured). The output dir captures full LLM transcripts (unstructured). They're complementary but should have a clear relationship — perhaps the state log entry includes a pointer to the transcript file.

4. **Should the model be configurable per-step or globally?** Both. Global `options.claude_model` with per-step override, following the same pattern as `timeout` and `max_retries`.

5. **Cost tracking?** Claude API returns token usage. Should the state log capture this? Useful for budgeting workflows.

---

## Visualization of Claude CLI Output

Barnum's job is to orchestrate the workflow and write output files. Visualization is explicitly out of scope for Barnum. But since `output_dir` produces structured JSON transcripts, users need tools to make sense of them.

### What Barnum produces

Each Claude invocation writes a JSON file to `output_dir`. The state log (NDJSON at `state_log_path`) captures the full task tree with timing, outcomes, and parent-child relationships.

Between these two, you have:
- **Task tree** (from state log): which tasks ran, in what order, parent-child relationships, success/failure
- **LLM transcripts** (from output dir): full prompts, responses, token usage, timing

### Suggested external tools

#### 1. **jq + command line**

The simplest option. NDJSON is jq's native format.

```bash
# Show all completed tasks with their step names
jq 'select(.kind == "TaskCompleted") | {task_id, outcome}' state.ndjson

# Show task tree (parent-child relationships)
jq 'select(.kind == "TaskSubmitted") | {id: .task_id, step, parent: .parent_id}' state.ndjson

# Total tokens across all Claude invocations
jq -s '[.[].usage.input_tokens // 0] | add' outputs/*.json

# Find failed tasks
jq 'select(.kind == "TaskCompleted" and .outcome.Failed)' state.ndjson
```

#### 2. **[fx](https://github.com/antonmedv/fx) or [jless](https://github.com/PaulJuliusMartinez/jless)**

Interactive JSON explorers. Point them at an output file to browse the full transcript:

```bash
jless outputs/task-0001-Analyze.json
fx state.ndjson
```

#### 3. **[Visidata](https://www.visidata.org/)**

Terminal spreadsheet that reads NDJSON natively. Good for tabular views of the state log:

```bash
vd state.ndjson
```

Shows all tasks in a sortable/filterable table. Can group by step name, filter by outcome, etc.

#### 4. **Custom HTML viewer (one-shot generation)**

A user could ask Claude to generate a single-file HTML visualization from their output files. No framework needed — just inline JS that reads the JSON files and renders a tree/timeline.

This is the "use Claude to build the tool" approach. Since the output format is well-defined JSON, Claude can easily generate a visualization tailored to the specific workflow.

#### 5. **Grafana / observability stack**

For production workflows, export the state log to an observability platform:
- State log entries become traces (each task is a span)
- Parent-child relationships map to span hierarchy
- Token usage and timing become metrics

A post-hook or finally-hook could push entries to a collector. But this is heavy — only relevant for production deployments.

### What Barnum should do (and not do)

**Do:**
- Write structured, well-documented JSON to `output_dir` and `state_log_path`
- Include enough metadata (timestamps, task IDs, parent IDs, token usage) that any tool can build a useful view
- Document the output format clearly so tool authors know what to expect

**Don't:**
- Build a web UI
- Bundle a visualizer
- Add a `barnum visualize` command
- Take a dependency on any rendering library

Barnum is a workflow engine. It produces structured output. Visualization is the user's problem, and the NDJSON + JSON format makes it trivially consumable by existing tools.
