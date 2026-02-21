# Future Directions

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

- Wrapper around `gsd submit` with timeout
- If submit process dies, multiplexer should detect and requeue
- Configurable per-task timeouts
- Distinguish between task timeout vs agent death

## GSD JSON Runner

Create a `gsd` binary that accepts JSON configuration for common cases:

```bash
gsd run config.json
```

Config structure (rough sketch):
```json
{
  "tasks": [...],
  "transitions": {
    "TaskA": ["TaskB", "TaskC"]  // Valid state transitions
  },
  "runtime_checks": true
}
```

- Tasks as opaque JSON blobs with well-known keys
- Runtime validation of state transitions
- The current multiplexer binary becomes the low-level tool
- `gsd` becomes the user-facing orchestrator

## Binary Naming

Current plan:
- `multiplexer` - low-level daemon (current `gsd_multiplexer` binary)
- `gsd` - high-level JSON-based orchestrator (future)
