# Testing Philosophy

## CLI-Only Testing

All tests should interact with troupe through CLI invocations only. This ensures:

1. Tests exercise the same code paths as real usage
2. The CLI becomes the source of truth for correctness
3. Internal implementation details can change without breaking tests

## No Manual Synchronization

Tests should NOT need to manually synchronize or wait for agents to be "ready". The CLI commands handle all synchronization internally:

- `submit_task` blocks until a response is received (via socket or file polling)
- `get_task` blocks until a task is available
- The daemon queues tasks automatically when no workers are available

This means tests can simply:
1. Start the daemon
2. Start agents
3. Submit tasks

The daemon will queue tasks submitted before agents register, and agents will pick them up once they're ready. No `wait_ready()`, no `thread::sleep()`, no polling.

## Example Pattern

```rust
// Start daemon
let _pool = TroupeHandle::start(&pool, test_name);

// Start agent (immediately starts listening for tasks)
let agent = TestAgent::echo(&pool, "agent-1", Duration::from_millis(5), test_name);

// Submit task - blocks until processed
let response = submit_with_mode(&pool, payload, data_source, notify_method)?;

// Verify response
assert!(matches!(response, Response::Processed { .. }));

// Cleanup
agent.stop();
```

No synchronization primitives needed. The CLI handles it.
