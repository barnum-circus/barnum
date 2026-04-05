# Handler panic recovery

## Motivation

The async dispatch model (engine dispatches tasks, event loop runs handlers, delivers results back) can hide failures. If a handler throws/panics and the event loop doesn't catch it, the engine never receives a `complete()` call for that task. Parent frames wait forever. The workflow hangs silently.

This is an audit task: verify that every handler failure path is caught and surfaced, not a new feature.

## What to verify

1. **TypeScript handler throws** — does the event loop catch the error and propagate it to the workflow?
2. **Rust builtin returns `Err`** — `execute_builtin` returns `Result`, but is the error path actually handled in the event loop dispatch?
3. **Subprocess crashes** — if the TS handler process exits non-zero, does the event loop detect this?
4. **No silent hangs** — if any of the above happen, does the workflow terminate with an error (not hang waiting for a completion that never arrives)?

## What to check in the event loop

The event loop (`barnum_event_loop`) dispatches handlers and delivers results. For each dispatch path, verify:

- Handler execution is wrapped in error handling (try/catch, Result propagation)
- On failure, the workflow terminates with a clear error including handler identity and error message
- No task ID is left orphaned in `task_to_parent` without a corresponding completion or failure

## Future: engine-level `fail()` method

If the audit reveals gaps, the engine may need a `fail(task_id, error)` method alongside `complete(task_id, value)` that propagates errors through the frame tree. But this is design work for later — the immediate goal is just ensuring failures aren't swallowed.
