# Future Directions

## Iterator Yielded Items

Currently `TaskRunner` yields `&mut Ctx` after each task completion. Could also yield a value from `process`:

```rust
trait QueueItem<Ctx> {
    type Yield = ();  // Default to unit
    // ...
    fn process(...) -> ProcessResult<Self::NextTasks, Self::Yield>;
}

struct ProcessResult<Tasks, Yield> {
    tasks: Tasks,
    yield_value: Yield,
}
```

This would allow the iterator to yield meaningful data per task completion without requiring context inspection.

## Step Prioritization

Steps could have priority weights affecting dispatch order. Higher priority tasks get processed first when multiple are queued. Useful for:

- Critical path optimization
- Background vs foreground work
- Resource-intensive vs lightweight tasks

## Streaming Support

Currently `QueueItem::start` returns a `Command`. Should support streaming responses:

```rust
enum TaskExecution {
    Command(Command),
    Stream(BoxedStream<...>),
    Local(BoxFuture<...>),  // Handle locally without spawning
}
```

Or at minimum, make Command optional for local-only processing.

## Fan-in / Reduce Pattern

We have fan-out (tasks spawn more tasks) but no built-in reduce. Example test case:

```rust
// Two tasks processed in random order:
// 1. LookupFirstName { user_id }
// 2. LookupLastName { user_id }
//
// In process, store partial results in context.
// When we have both first + last name for a user,
// emit ProcessFullName { first, last } task.
```

Need unit tests exercising context-based coordination between tasks.

### JSON Config Approaches (Not Yet Decided)

**Option 1: Barrier/Join Step**
```json
{
  "name": "FullNameReady",
  "join": ["FetchFirstName", "FetchLastName"],
  "instructions": "Both names fetched. Results in $JOIN_RESULTS."
}
```

**Option 2: Accumulator in Value**
Tasks pass accumulator through `value` - state travels with tasks, no daemon tracking:
```json
// FetchFirstName returns:
[{"kind": "FetchLastName", "value": {"first": "John", "pending": ["last"]}}]
// FetchLastName sees pending empty, emits ProcessFullName
```

**Option 3: Entity-Scoped Context**
```json
{
  "name": "FetchFirstName",
  "context_key": "user:{{user_id}}",
  "on_context_complete": ["first", "last"],
  "then": "ProcessFullName"
}
```

Option 2 is most "JSON-native" but requires agents to understand the accumulator pattern. Need more real-world use cases before committing.

## Sequential Processing by Key

Sometimes you want multiple tasks for the same entity (e.g., file) to be processed sequentially rather than in parallel. Example: three refactors for `main.rs` should be applied one at a time with commits between each.

### Workaround: Self-Looping Step

This is achievable today by having the agent pass remaining items through the value:

```json
{
  "steps": [
    {
      "name": "Analyze",
      "instructions": "Analyze files. Return list of refactors grouped by file.",
      "next": ["ProcessRefactorList"]
    },
    {
      "name": "ProcessRefactorList",
      "instructions": "Apply first refactor from the list. Return remaining list (or empty to finish).",
      "next": ["ProcessRefactorList", "Commit", "Done"]
    },
    {
      "name": "Commit",
      "instructions": "Commit changes for the file. Could also re-analyze.",
      "next": ["ProcessRefactorList", "Done"]
    },
    {
      "name": "Done",
      "next": []
    }
  ]
}
```

The agent receives `{"refactors": [...], "current_file": "..."}` and:
1. Applies the first refactor for `current_file`
2. Returns the same task with one fewer refactor
3. When the file's refactors are exhausted, emits `Commit` or moves to next file

**Limitation**: Requires agents to understand the list-processing pattern. The daemon has no concept of "key" for sequential ordering.

### Potential Future Primitive

Could add `sequential_key` to steps:
```json
{
  "name": "ApplyRefactor",
  "sequential_key": "{{value.file}}",
  "instructions": "..."
}
```

Tasks with the same key value would be queued and processed one at a time. This would require daemon-side tracking of in-flight keys.

## Durability

Currently no durability guarantees. Tasks in flight are lost on crash. Document this clearly and consider:

- Optional persistence layer
- At-least-once vs at-most-once semantics
- Checkpoint/resume support

## Timeout Handling

Need timeout support for tasks. Considerations:

- Wrapper around `agent_pool submit` with timeout
- If submit process dies, agent_pool should detect and requeue
- Configurable per-task timeouts
- Distinguish between task timeout vs agent death

## Granular Retry Options

**Status: Implemented** in `crates/gsd_config/src/config.rs`.

Global options `retry_on_timeout` and `retry_on_invalid_response` (both default `true`) can be overridden per-step via `StepOptions`. Use `EffectiveOptions::resolve()` to merge.

## No-IPC Mode

For sandboxed environments where Unix sockets are blocked, implement file-based task submission:

- Submit writes to a `pending/` folder
- Daemon polls for new files
- Response written back to same folder
- Submit blocks until response appears

## GSD JSON Runner

**Status: Implemented** in `crates/gsd_config/` (library) and `crates/gsd_cli/` (binary).

The `gsd` binary accepts JSON configuration:

```bash
gsd run config.json --root /tmp/pool --initial '[{"kind": "Start", "value": {}}]'
gsd docs config.json
gsd validate config.json
```

See `crates/gsd_config/README.md` for full documentation.

## Binary vs Library Mode

There's a fundamental tension between two usage modes:

**Binary mode** (CLI): The daemon runs as a standalone process, inherently stateful, with a never-returning main loop. Process lifecycle is managed externally (systemd, supervisord, etc.).

**Library mode** (embedded): The daemon is spawned within another Rust program via `spawn()`, returning a `DaemonHandle` for programmatic control (pause, resume, shutdown).

Current design supports both:
- `run()` for binary mode (never returns on success, `Infallible` return type)
- `spawn()` for library mode (returns handle for control)

Future considerations:
- Should library mode support async (`spawn_async()` returning a future)?
- Should there be a way to inject custom event handlers (hooks for task lifecycle)?
- Could the binary mode delegate to library mode internally for code reuse?

## Agent Differentiation (Non-Goal)

**Explicitly NOT planned**: Agent prioritization, tagging, or differentiation.

Currently all agents are equal - tasks are dispatched to any available agent. We do not plan to add:
- Agent capabilities/tags (e.g., "this agent handles GPU tasks")
- Task routing based on agent properties
- Priority queues for different task types
- Agent affinity or stickiness

Why not:
- Adds significant complexity to the dispatch logic
- Most use cases don't need it (homogeneous worker pools)
- Can be implemented in userspace if needed (separate pools per capability)
- Goes against the "simple pool of identical workers" model

If differentiation is needed, consider running multiple `agent_pool` instances, one per agent type, and routing tasks appropriately at submission time.

## Step Cleanup/Post-Processing

Steps might want to run cleanup actions after completion, regardless of which next step is chosen. Use cases:

- Commit changes after each step
- Run linters/formatters after code modifications
- Log metrics or telemetry
- Release resources or locks

### Potential Approaches

**Option 1: `on_complete` field**
```json
{
  "name": "Implement",
  "on_complete": "./scripts/lint-and-format.sh",
  "next": ["Test", "Done"]
}
```

**Option 2: Lifecycle hooks**
```json
{
  "hooks": {
    "after_step": "./scripts/cleanup.sh"
  },
  "steps": [...]
}
```

**Option 3: Leave to agents**
Agents can include cleanup in their response logic. Simpler but requires agent awareness.

Not yet decided - need more use cases to understand the right abstraction.

## Hung Agent Detection

Agents can hang (infinite loops, deadlocks, waiting on unavailable resources). The daemon should detect this and handle it gracefully:

- Per-task timeout configuration
- Daemon monitors in-flight task duration
- On timeout: respond to submitter with `NotProcessed { reason: Timeout }`
- Agent cleanup: kill hung agent process, mark agent as unhealthy
- Agent recovery: option to auto-restart agents or remove them from pool

This is distinct from task-level timeouts (handled by the submit wrapper). The daemon needs to know when an agent itself is stuck, not just when a particular task is taking too long.

## Agent Auto-Deregistration on Repeated Timeouts

Track timeouts per agent. After N timeouts (e.g., 3), auto-deregister the agent from the pool:

- Daemon tracks timeout count per agent
- On timeout, increment counter
- At threshold (configurable, default 3), remove agent directory
- `get_task` should return an error if the agent was deregistered, explaining why
- Agent can re-register to reset their timeout count and try again

This prevents slow/broken agents from clogging the pool indefinitely.

## Low-Level File Protocol Documentation

Currently the agent protocol documentation only covers the CLI commands (`get_task`, `deregister_agent`). The underlying file protocol (`task.json`, `response.json`) is an implementation detail.

If we need to support agents that can't use the CLI (e.g., non-Rust agents, embedded systems), we should document the raw file protocol:

- Agent creates directory in `agents/`
- Daemon writes `task.json` when task is assigned
- Agent writes `response.json` when done
- Daemon cleans up both files

For now, the CLI abstracts this away. Expose only if there's a concrete use case.

## Full Socket-Based Protocol

Remove filesystem-based IPC entirely:

- Pool ID becomes just a PID or similar in-memory identifier
- All communication via socket (daemon ↔ agents, daemon ↔ submitters)
- `get_task` blocks on socket until task assigned
- `complete_task` sends response over socket
- No temp files, no directory watching
- Simpler, faster, easier to reason about

The CLI commands (`register`, `get_task`, `complete_task`) already abstract away the filesystem, so this would be a transparent change for agents using the CLI.
