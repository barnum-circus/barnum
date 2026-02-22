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
// In cleanup, store partial results in context.
// When we have both first + last name for a user,
// emit ProcessFullName { first, last } task.
```

Need unit tests exercising context-based coordination between tasks.

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

## No-IPC Mode

For sandboxed environments where Unix sockets are blocked, implement file-based task submission:

- Submit writes to a `pending/` folder
- Daemon polls for new files
- Response written back to same folder
- Submit blocks until response appears

## GSD JSON Runner

**Status: Implemented** in `crates/gsd/`.

The `gsd` binary accepts JSON configuration:

```bash
gsd run config.json --root /tmp/pool --initial '[{"kind": "Start", "value": {}}]'
gsd docs config.json
gsd validate config.json
```

See `crates/gsd/DESIGN.md` for full documentation.

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

## Hung Agent Detection

Agents can hang (infinite loops, deadlocks, waiting on unavailable resources). The daemon should detect this and handle it gracefully:

- Per-task timeout configuration
- Daemon monitors in-flight task duration
- On timeout: respond to submitter with `NotProcessed { reason: Timeout }`
- Agent cleanup: kill hung agent process, mark agent as unhealthy
- Agent recovery: option to auto-restart agents or remove them from pool

This is distinct from task-level timeouts (handled by the submit wrapper). The daemon needs to know when an agent itself is stuck, not just when a particular task is taking too long.
