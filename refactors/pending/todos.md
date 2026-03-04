# To-Dos and Future Directions

## Most Important

1. ~~**Command Steps**~~ ✓ COMPLETE - Steps can use `action.kind = "Command"` with a `script` field to execute bash locally instead of dispatching to an agent pool.

2. **Multi-Pool Task Routing** - Allow different steps to be routed to different agent pools. This enables heterogeneous workflows where some tasks require specialized agents (e.g., code review agents vs. implementation agents) or where command execution happens in a separate pool from AI reasoning.

---

## Command Agent Improvements

### Reconnect on Timeout

When the command agent times out (e.g., due to heartbeat failure), it should automatically reconnect instead of exiting. With anonymous workers, this means just calling `get_task` again.

**Current behavior:**
- Agent receives Kicked message or times out → exits
- User must manually restart the agent

**Desired behavior:**
- On timeout/kick, loop back to `get_task` instead of exiting
- Seamlessly continues processing tasks

### Command Timeout

The command agent should have its own configurable timeout for executing commands, separate from the daemon's heartbeat timeout.

**Problem:**
- Daemon heartbeat timeout is ~60s
- Some commands take longer than that
- Agent gets kicked while command is still running

**Solution:**
- Add `--timeout` flag to command agent
- Execute commands with timeout wrapper
- If command times out, return error response instead of hanging

---

## Speculative: GSD Direct Library Integration

**Status: Speculative**

Currently GSD submits tasks by spawning `agent_pool submit_task` CLI processes. Each subprocess creates its own inotify watcher, which can exhaust the `max_user_instances` limit (typically 128) when submitting many tasks concurrently.

**Current approach:**
- GSD spawns subprocess per task submission
- Each subprocess creates inotify watcher
- `max_concurrency=20` mitigates but doesn't eliminate issue

**Better approach:**
- GSD links against agent_pool library directly
- Single watcher shared across all submissions
- No subprocess overhead

This would also improve performance (no CLI spawn overhead) and simplify the codebase.

---

## Re-enable Multi-Threaded Tests

**Status: TODO**

Tests currently run with `--test-threads=1` in CI due to timeout issues when running in parallel. The root cause is CLI spawn overhead compounding with parallel test execution - each test spawns multiple CLI processes (daemon, agents, submitters), and running many tests simultaneously overwhelms the system.

**Investigation needed:**
- Profile CLI spawn overhead to understand the bottleneck
- Consider connection pooling or persistent daemon connections
- Look into test isolation improvements (separate pool per test already done)
- May need to reduce per-test CLI spawns or batch operations

---

## Investigate KQueue for macOS Performance

**Status: TODO**

The `notify` crate uses FSEvents on macOS by default. KQueue might provide better performance for our use case (watching specific files rather than directory trees).

**Investigation needed:**
- Benchmark FSEvents vs KQueue for our access patterns
- Check if `notify` supports KQueue backend selection
- Evaluate if the complexity is worth potential gains

---

## Remove Remaining Polling

**Status: MOSTLY COMPLETE**

~~2. **File-based submission response wait**~~ ✓ DONE - `submit/file.rs` now uses `VerifiedWatcher`.

Remaining:
1. **`wait_for_pool_ready`** in `crates/agent_pool/src/client/mod.rs` - spins with `thread::sleep(10ms)` waiting for pool directory to exist. Low priority since this is brief at startup.

---

## ~~Agent Cleanup pkill Pattern is Too Broad~~ OBSOLETE

**Status: OBSOLETE** - The `register` command no longer exists. Anonymous workers use `get_task` which doesn't leave long-running processes in the same way.

---

## Shutdown and Cleanup Behavior

**Status: PARTIALLY COMPLETE**

Cleanup is now automatic on startup and graceful shutdown:

**Implemented (Phase 5 from INOTIFY_RACE_ANALYSIS.md):**
- `cleanup_pool_state()` runs on startup to clean stale state from crashed daemons
- `PoolStateCleanup` guard cleans up on graceful shutdown or panic
- Removes: status file, all files in submissions/, all directories in agents/, canary files

**Remaining questions:**
- Signal handlers (SIGINT/SIGTERM) don't run Drop guards - cleanup happens on next startup instead
- For debugging crashed daemons, users would need to disable cleanup (no flag for this yet)
- The `stop()` command sends SIGTERM which doesn't trigger Drop - cleanup happens on next startup

**Related:**
- `stop()` in `client/stop.rs` - sends SIGTERM to PID from lock file
- `cleanup_stopped()` in `pool.rs` - removes directories for non-running pools (backup cleanup)
- Test harnesses use `setup_test_dir()` which already cleans up

---

## Socket-Based Task Submissions

**Status: COMPLETE** - Socket-based task submissions work. The `submit_task` command uses sockets by default (`--notify socket`), with file-based as fallback (`--notify file`) for sandboxed environments.

---

## Socket-Based Agent Notifications

**Status: NOT IMPLEMENTED** - Agent command `get_task` is file-based only.

**Potential benefit:** Instead of file-based task dispatch, the daemon could push tasks to agents via socket. This would be faster and more efficient.

**What needs to be done:**
1. Add `--notify socket` flag to `get_task` command
2. Agent opens socket connection to daemon
3. Daemon pushes tasks to connected agents instead of writing task files
4. Agent sends responses over socket instead of writing response files

**Considerations:**
- File-based should remain the default (works in sandboxed environments)
- Socket mode is opt-in for when sockets are available
- Need to handle reconnection if socket drops

---

## Document: Inaccessible Filesystem Edge Case

When implementing file reference payload support, add an internal comment or doc explaining:

Currently we assume submitter, daemon, and agents share the same filesystem. File reference works because everyone can read the path. In a future where the daemon can't access the submitter's filesystem (e.g., different machines without shared storage), the CLI would need to detect this and automatically read the file, sending content inline instead of the path.

This edge case is documented in TRANSPORT_ABSTRACTION.md under "Edge Case: Inaccessible Filesystems" but should also be noted in the code when file reference is implemented.

---

## ~~Flaky Test: sequential_tasks_same_agent~~ RESOLVED

**Status: RESOLVED** - Test has been removed.

---

## ~~Agent --continue Flag~~ OBSOLETE

**Status: OBSOLETE** - With anonymous workers, agents don't have persistent state. Each `get_task` call creates a new worker UUID. If an agent disconnects mid-task, the task times out and can be resubmitted.

---

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

**Status: COMPLETE** - File-based submission works via `--notify file` flag.

- Submit writes to `submissions/` folder
- Daemon uses filesystem watcher (not polling)
- Response written back to same folder
- Submit blocks until response appears (via watcher)

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

Currently the agent protocol documentation only covers the CLI command (`get_task`). The underlying file protocol is an implementation detail.

If we need to support agents that can't use the CLI (e.g., non-Rust agents, embedded systems), we should document the raw file protocol:

- Worker calls `get_task` which creates a worker directory with UUID
- Daemon writes task to worker's task file when work is available
- Worker writes response to the response file path provided in the task
- Worker calls `get_task` again for next task

For now, the CLI abstracts this away. Expose only if there's a concrete use case.

## Default Entry Step

Currently, `gsd run` requires `--initial` to specify starting tasks. For simple workflows with a single entry point, this is verbose:

```bash
gsd run config.json --pool /tmp/pool --initial '[{"kind": "Start", "value": {}}]'
```

Could support a `default_step` in config that makes `--initial` optional:

```json
{
  "default_step": "Start",
  "steps": [
    {"name": "Start", "next": ["Analyze"]},
    ...
  ]
}
```

Then `gsd run config.json --pool /tmp/pool` would automatically start with `[{"kind": "Start", "value": {}}]`.

Rules:
- If `default_step` is set and `--initial` is provided, use `--initial` (explicit wins)
- If `default_step` is set and no `--initial`, use the default step with empty value `{}`
- If no `default_step` and no `--initial`, error (current behavior)

The default step should probably require `value_schema` to either be absent or accept an empty object.

## Agent Health Checks

**Status: COMPLETE** - see `refactors/past/HEALTH_CHECK_PLAN.md`.

Task-based ping-pong health checks (Heartbeat messages). Benefits:
- Periodic heartbeats to idle workers detect disconnected workers
- Workers can recover from timeout by simply calling `get_task` again

The `get_task` response includes `kind` which can be:
- `Task` - real work to do
- `Heartbeat` - liveness check, respond with any JSON
- `Kicked` - worker was removed (e.g., timeout), call `get_task` again to reconnect

## Typestate Pattern for State Transitions

We introduced a pattern in `InFlight::complete(self, output)` where the state transition method lives on the inner type and consumes `self`. This couples the state transition with the action it enables (sending the response), making it harder to forget one or the other.

Look for other places where state and actions are decoupled and could benefit from this pattern:

- `AgentState` transitions (idle → busy, busy → idle)
- `Task` lifecycle (pending → dispatched → completed)
- `DaemonHandle` state (running → paused → shutdown)

The pattern: instead of separately mutating state and performing I/O, have a method on the state type that consumes it and returns the next state while performing the associated action.

```rust
// Before: decoupled state and action
agent.status = AgentStatus::Idle;
send_response(respond_to, &response)?;

// After: coupled via typestate
let idle = busy_agent.complete(output)?;  // consumes BusyAgent, returns IdleAgent, sends response
```

This is a low-priority refactor - apply opportunistically when touching related code.

---

## Multi-Pool Support in GSD

GSD should support orchestrating agents across multiple pools. Use case: integrating command pools (for running shell commands outside sandbox) with agent pools (for AI agents).

Example workflow:
```json
{
  "pools": {
    "agents": "/tmp/agent-pool",
    "commands": "/tmp/cmd-pool"
  },
  "steps": [
    {
      "name": "Analyze",
      "pool": "agents",
      "instructions": "Analyze the codebase"
    },
    {
      "name": "RunTests",
      "pool": "commands",
      "instructions": "cargo test --workspace"
    }
  ]
}
```

Each step specifies which pool to dispatch to. This enables:
- AI agents for creative/analytical work
- Command pools for deterministic shell operations
- Mixed workflows that combine both

Implementation considerations:
- Multiple pool connections in the runner
- Pool-specific configuration (timeouts, retries)
- Cross-pool dependencies and data passing

---

## JSON Schema Output Command

Add a `gsd schema` command that outputs the JSON schema for the GSD configuration file. This enables:

- IDE autocomplete via JSON schema support
- Validation in external tools
- Documentation generation
- AI assistants to understand the config format

```bash
gsd schema > gsd-config.schema.json
```

The schema should be derived from the Rust types in `gsd_config::Config` and friends, possibly using `schemars` crate.

---

## Initial Step with CLI Data

Add support for an `initial` step in the config that receives its data from the command line. This simplifies the common case where a workflow has a single entry point.

**Config format:**
```json
{
  "steps": [
    {
      "name": "Start",
      "initial": true,
      "next": ["Process"]
    }
  ]
}
```

**CLI usage:**
```bash
# Data provided on command line goes to the initial step
gsd run config.json --pool /tmp/pool --data '{"user_id": 123}'
```

Rules:
- At most one step can have `initial: true`
- If `initial` is set, `--initial-steps` is no longer required
- `--data` provides the `value` for the initial step
- If both `initial` step and `--initial-steps` are provided, error (or `--initial-steps` wins?)

This replaces the verbose:
```bash
gsd run config.json --pool /tmp/pool --initial '[{"kind": "Start", "value": {"user_id": 123}}]'
```

With:
```bash
gsd run config.json --pool /tmp/pool --data '{"user_id": 123}'
```

---

## Full Socket-Based Protocol

Remove filesystem-based IPC entirely:

- Pool ID becomes just a PID or similar in-memory identifier
- All communication via socket (daemon ↔ agents, daemon ↔ submitters)
- `get_task` blocks on socket until task assigned
- Response sent over socket instead of writing file
- No temp files, no directory watching
- Simpler, faster, easier to reason about

The CLI commands already abstract away the filesystem, so this would be a transparent change for agents using the CLI.

---

## CI Step Time Limits

Add a 5-minute timeout to each step in CI workflows. This prevents hung builds from consuming resources and provides faster feedback when something is stuck.

---

## ~~Parallelize Pre-Commit Hook Checks~~ ✓ DONE

**Status: COMPLETE** - Pre-commit hook runs fmt first, then check/clippy/test/udeps in parallel.

---

## ~~Agent CLI: Respond-and-Deregister / Abort Support~~ OBSOLETE

**Status: OBSOLETE** - With anonymous workers, there's no `deregister_agent` or `next_task`. Agents just write to the response file and call `get_task` again. To stop, simply don't call `get_task`. To abort, write an error response and call `get_task` again.

---

## ~~Rethink Agent Deregistration~~ OBSOLETE

**Status: OBSOLETE** - With anonymous workers, there's no persistent agent identity or deregistration. Workers are ephemeral: they call `get_task`, do work, write response, repeat. To stop, just don't call `get_task` again. The daemon tracks workers by their current task, not by identity.

---

## Track Completed Submissions to Prevent Re-Registration

**Status: NEEDS IMPLEMENTATION**

There's a bug where completed submission directories can be re-registered by the file watcher, causing agents to process duplicate tasks. The current guards are:

1. `path_to_id` check - if path is already registered, skip
2. `response.json` exists check - if response exists, skip

But after `finish()` completes a task, it removes the path from `path_to_id`. If a delayed FS event fires after this, the first check passes. The second check SHOULD catch it (response.json was written), but somehow duplicates still occur.

**Proposed fix:** Track completed submission paths in a separate set that's never cleared:

```rust
// In io_loop state
completed_submissions: HashSet<PathBuf>,

// In finish()
if let Transport::Directory(ref path) = transport {
    completed_submissions.insert(path.clone());
}

// In register_pending_task
if completed_submissions.contains(submission_dir) {
    return;  // Already completed, never re-register
}
```

This is simpler and more robust than relying on filesystem state. A completed submission is completed forever - no need to check file existence.

**Alternative:** Instead of a HashSet, track completed UUIDs in a `HashSet<String>` keyed by the UUID portion of the path. This would be more memory-efficient for long-running daemons.

**Questions:**
- Should this set ever be pruned? (Probably not for typical daemon lifetimes)
- Is there a memory concern for extremely long-running daemons with millions of submissions?
- Should we also track "in-flight" submissions to distinguish "currently being processed" from "completed"?

---

## wait_for_pool_ready Spins for Directory

**Status: LOW PRIORITY**

In `crates/agent_pool/src/client/mod.rs`, `wait_for_pool_ready` spins with `thread::sleep(10ms)` waiting for the pool directory to exist. This is because the daemon subprocess needs time to create the directory after being spawned.

Should use a watcher on the parent directory (`/tmp/gsd/`) instead of spinning. Low priority because the spin is short-lived (directory is created quickly) and only happens during pool startup.

---

## Return Step Cardinality Validation

**Status: TODO**

Add cardinality constraints for the `next` array that agents return. Currently agents can return any number of tasks for any next step. We should support constraints like:

- **Exactly 0**: `[]` - terminal, no further tasks
- **Exactly 1**: Must return exactly one task of this type
- **0 or 1**: Optional single task
- **1 or more**: At least one task required
- **0 or more**: Any number (current default behavior)

Example config syntax (TBD):
```json
{
  "name": "Analyze",
  "next": [
    {"step": "ProcessFile", "cardinality": "1+"},
    {"step": "Done", "cardinality": "0-1"}
  ]
}
```

Or simpler syntax:
```json
{
  "name": "Analyze",
  "next": {
    "ProcessFile": "1+",
    "Done": "0-1"
  }
}
```

Benefits:
- Catch agent errors early (wrong number of tasks)
- Self-documenting workflows
- Enable optimization (daemon knows fan-out patterns in advance)

---

## Runtime Workflow Graph (Execution History)

**Status: TODO**

Generate a workflow graph based on an actual run, not just the static config. This would show:

1. **Duplicate steps**: If a step ran multiple times (e.g., ProcessRefactorList for each file), show each instance
2. **Parent-child relationships**: Arrows from the step that created a task to the step that processed it
3. **Agent attribution**: Which agent(s) worked on each step, accounting for retries
4. **Timing/ordering**: Visual indication of execution order

Use cases:
- Debug complex workflows to see what actually happened
- Understand task fan-out patterns
- Identify bottlenecks (which steps took longest, which agents were busiest)

Implementation considerations:
- GSD runner would need to track execution history (task spawns, completions, agent assignments)
- New `gsd graph --from-history <history.json>` or similar command
- History could be written to a file during execution (`--trace execution.json`)
- Graph output could use DOT format with additional styling for duplicates/agents

---

## Rename Pool Directory from gsd to agent_pool

**Status: COMPLETE**

The default pool directory is now `/tmp/agent_pool/<pool_id>/`. The `--pool-root` CLI flag allows overriding this.

Changes made:
- Added `default_pool_root()` function in `pool.rs`
- Added `--pool-root` global CLI option
- Updated all functions to accept pool root as parameter
- Updated all doc references from `/tmp/gsd/` to `/tmp/agent_pool/`

---

## ~~GSD Pool Root Support~~ ✓ COMPLETE

**Status: COMPLETE**

The `gsd` CLI now has `--pool-root` global flag matching the `agent_pool` CLI:

```bash
# Absolute paths still work (backward compatible)
gsd run config.json --pool /custom/path/my-pool

# New: cleaner syntax with --pool-root
gsd run config.json --pool my-pool --pool-root /custom/path
```

Pool argument handling:
- Absolute paths (starting with `/`) are used directly
- Relative IDs (no slashes) are resolved relative to `pool_root`
- Relative paths with slashes are rejected with E058 error

---

## Explicit Response Schema Field on Task

**Status: TODO**

Currently, the JSON schema for valid responses is embedded in the instructions markdown. The agent sees something like:

```markdown
## Valid Responses

Value must match schema:

```json
{"type": "object", "properties": {...}}
```
```

This is hard for agents to parse programmatically. The schema should be an explicit field on the task JSON:

**Current task format:**
```json
{
  "kind": "Task",
  "task": {
    "instructions": "...markdown with schema embedded...",
    "data": {...}
  }
}
```

**Proposed:**
```json
{
  "kind": "Task",
  "task": {
    "instructions": "...",
    "data": {...},
    "response_schema": {
      "type": "array",
      "items": {
        "oneOf": [
          {"properties": {"kind": {"const": "NextStep"}, "value": {...}}},
          ...
        ]
      }
    }
  }
}
```

Benefits:
- Agents can validate their own responses before submitting
- Easier for AI agents to understand the expected output format
- Schema is machine-readable, not buried in markdown
- The field can be optional (null or absent) for steps without schema constraints

---

## GSD Task Monitoring via Log File

**Status: TODO**

Need a way to monitor active tasks while GSD is running. Currently there's no easy way to see what's happening.

**Proposed changes:**

1. **Always write a log file to the pool directory** - Remove the optional `--log` flag from GSD. Instead, always write to a well-known location like `<pool_root>/gsd.log` or `<pool_root>/gsd-<timestamp>.log`.

2. **Log active tasks** - The log should show:
   - Tasks submitted to agents
   - Tasks completed
   - Current in-flight tasks
   - Queue depth

3. **Easy monitoring** - Users can `tail -f <pool_root>/gsd.log` to watch progress in real-time.

This makes debugging and monitoring much simpler - just tail the log file in the pool directory.

---

## GSD Should Use CLI Instead of Rust API

**Status: COMPLETE**

GSD now invokes the CLI via `submit_via_cli()` which spawns `agent_pool submit_task`. See `crates/gsd_config/src/runner.rs`.

---

## wait_for_task Should Accept Cancellation Signal

**Status: REFACTOR PLANNED** - See `refactors/pending/CANCELLABLE_WAIT_FOR_TASK.md`

---

## Unify agent.rs with VerifiedWatcher

**Status: TODO**

The `agent.rs` module has its own watcher pattern (`create_watcher`, `verify_watcher_sync`, `wait_for_task`) that predates `VerifiedWatcher`. We should investigate whether to unify these.

**Key difference from client flows:**

The agent's task ready condition is more complex than just waiting for a file:
```rust
task.exists() && !response.exists()
```

This checks both that a task file appeared AND that the response file is gone (indicating the previous task was cleaned up).

**Options to investigate:**

1. **Pass a predicate function to VerifiedWatcher** - Add a `wait_for_predicate(f: impl Fn() -> bool)` method that checks the predicate on each event instead of checking for a specific file.

2. **Use VerifiedWatcher internally but keep agent API** - The agent API is designed for watcher reuse across multiple task waits. Could use VerifiedWatcher for the canary verification part but keep the custom task-ready logic.

3. **Simplify the task-ready check** - If we can guarantee that `task.json` only appears when the agent should process it (daemon always cleans up response before writing new task), we could simplify to just `wait_for(&task_path)`.

**Current state:**

The agent module already has proper canary verification and is event-driven (not polling). The main question is whether unification provides enough value to justify the API changes.

---

## ~~Annotate All Error Messages with Context~~ ✓ COMPLETE

**Status: COMPLETE**

All error messages now have:
1. **Unique error IDs** (E001-E057) for easy lookup
2. **Contextual information** (file paths, IDs, values)

Error ID ranges:
- E001-E004: verified_watcher.rs (atomic write, watcher disconnect)
- E005-E008: submit/file.rs (file-based submission)
- E009-E013: daemon/io.rs (ID/submission lookup, finish)
- E014-E021: runner.rs (pool paths, submit, wake, command)
- E022: cli_invoker (not found)
- E023-E026: stop.rs (daemon stop)
- E027-E028: transport.rs (socket not implemented)
- E029: lock.rs (daemon already running)
- E030-E036: daemon/wiring.rs (event loop, socket)
- E037-E043: submit/socket.rs (socket-based submission)
- E044-E047: verified_watcher.rs (watcher creation, timeouts)
- E048-E050: value_schema.rs (schema loading)
- E051-E057: gsd_cli/main.rs (config parsing/validation)

Example:
```
[E005] failed to canonicalize pool root /tmp/foo: No such file or directory (os error 2)
```

---

## Agent Pool Protocol --tag Parameter

**Status: TODO**

Add a `--tag` parameter to `agent_pool protocol` command (or the agent instructions) to specify which npm tag to use when running `pnpm dlx @gsd-now/agent-pool`.

Currently agents are manually told to use `@main` in the instructions. This should be a first-class parameter.

---

## CLI Invoker Version Checking

**Status: TODO**

The `cli_invoker` crate resolves how to invoke CLI tools (binary path, package manager dlx, etc.) but doesn't verify version compatibility.

**Issues:**

1. **Binary version mismatch** - When using a binary directly (env var, cargo workspace, node_modules), we should check `--version` first and warn if it differs from the current library version.

2. **Package manager version pinning** - When using `pnpm dlx @gsd-now/agent-pool` (or similar), we should include the current version: `pnpm dlx @gsd-now/agent-pool@0.1.0`. Otherwise we might get a different version than expected.

**Implementation:**

1. Add `VERSION` constant to `cli_invoker` (or pass it in via trait)
2. For binary invocation: run `<binary> --version`, parse output, warn if mismatch
3. For package manager invocation: append `@{VERSION}` to the package name in `prefix_args`

**Priority:** Low - not a blocker for initial release, but important for stability when multiple versions exist.
