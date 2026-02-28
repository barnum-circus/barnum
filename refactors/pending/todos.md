# To-Dos and Future Directions

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

**Status: TODO**

Some places still use polling instead of proper synchronization:

1. **`wait_for_pool_ready`** in `crates/agent_pool/src/client/mod.rs` - spins with `thread::sleep(10ms)` waiting for pool directory to exist
2. **File-based submission response wait** in `crates/agent_pool/src/client/submit_file.rs` - polls for `response.json` every 100ms

Both should use filesystem watchers instead of polling.

---

## Agent Cleanup pkill Pattern is Too Broad

**Status: NEEDS FIX**

The `start-cmd-pool.sh` and `start-cmd-agents.sh` scripts use `pkill -9 -f "agent_pool register --pool cmd"` to kill stale CLI subprocesses. This pattern is too broad - it will kill ALL `agent_pool register` processes for the `cmd` pool, including legitimate ones from other scripts.

**Options:**
1. Track child PIDs explicitly and kill those
2. Use process groups
3. Store PIDs in a file and kill from there
4. Accept the limitation (cmd pool is usually one set of agents)

Low priority - the cmd pool is typically used by one set of agents at a time.

---

## Shutdown and Cleanup Behavior

**Status: NEEDS DESIGN**

Currently when the daemon stops (via SIGTERM/Ctrl+C or `agent_pool stop`), the pool directory remains with stale state. Users must manually run `agent_pool cleanup` (the pool directory is automatically wiped on restart).

**Desired behavior:**
- Ctrl+C (SIGINT) should clean up the pool directory before exiting
- `agent_pool stop` should kill the process AND remove the directory
- Think comprehensively about what "shutdown" means for:
  - CLI daemon (killed via signal)
  - Embedded daemon (`DaemonHandle::shutdown()`)
  - Tests (need clean state between runs)

**Questions to answer:**
- Should cleanup be default or opt-in?
- What about debugging? Users might want to inspect state after crash
- How to handle signal handlers safely (can't do complex ops in signal handlers)?
- Should `DaemonHandle::shutdown()` also clean up, or is that test harness's job?

**Related:**
- `stop()` in `client/stop.rs` - sends SIGTERM to PID from lock file
- `cleanup_stopped()` in `pool.rs` - removes directories for non-running pools
- Test harnesses use `setup_test_dir()` which already cleans up

---

## Socket-Based Task Submissions

**Status: COMPLETE** - Socket-based task submissions work. The `submit_task` command uses sockets by default (`--notify socket`), with file-based as fallback (`--notify file`) for sandboxed environments.

---

## Socket-Based Agent Notifications

**Status: NOT IMPLEMENTED** - Agent commands (`get_task`, `next_task`) are file-based only. No `--notify` flag exists on these commands.

**Potential benefit:** Instead of agents polling for `task.json`, the daemon could push tasks to agents via socket. This would be faster and more efficient.

**What needs to be done:**
1. Add `--notify socket` flag to `get_task` and `next_task` commands
2. Agent opens socket connection to daemon
3. Daemon pushes tasks to connected agents instead of writing `task.json`
4. Agent sends responses over socket instead of writing `response.json`

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

## Flaky Test: sequential_tasks_same_agent

The test `crates/agent_pool/tests/single_agent_queue.rs::sequential_tasks_same_agent` is flaky. It bypasses the daemon and manually writes to task.json/response.json, which creates race conditions between the test's file operations and the TestAgent's polling loop.

Options:
1. **Use proper synchronization** - Have TestAgent signal when it's ready for the next task
2. **Use the daemon** - Rewrite to use AgentPoolHandle instead of manual file protocol
3. **Add retry logic** - Poll for the expected response instead of asserting after fixed sleep
4. **Delete the test** - The same behavior is tested via the daemon in other tests

The test is brittle by design (testing raw file protocol) so option 2 or 4 may be best.

---

## Agent --continue Flag

When an agent starts with a specific ID via `get_task --name <ID>`, we might want to distinguish between:

1. **Fresh start**: This is a brand new agent with this ID
2. **Resume**: This agent existed before and we want to continue where we left off

Potential approach:
- Add `--continue` flag to `get_task`
- Without `--continue`: If agent directory exists, error (or delete it first)
- With `--continue`: If agent directory exists, resume (check for pending task.json, etc.)

Use cases:
- Agent process restarts mid-task and wants to pick up where it left off
- Graceful recovery from crashes
- Session continuity for long-running agents

Open questions:
- What exactly constitutes "state" to continue? Just the directory existence, or pending task/response files?
- Should the daemon track agent state beyond the directory?
- How does this interact with keepalives? (Probably: if continuing, skip initial keepalive?)

This needs more thought before implementation.

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

Task-based ping-pong health checks. Benefits:
- Initial health check gets tool-use approvals out of the way
- Periodic health checks to idle agents detect disconnected agents
- Agents can recover from timeout by simply calling `get_task` again

### Health Check Visibility

The `get_task` CLI has `--auto-health-check` (default: **false**).

**Why agents should see health checks (default):**
- Health checks require the agent to actively respond
- This prevents Claude from deciding "nothing's happening, I'll leave"
- The ping-pong provides forward progress that keeps agents engaged

```bash
# Default: agent sees and must respond to heartbeats
task=$(agent_pool get_task --pool $POOL --name $NAME)
# task.kind can be "Task", "Heartbeat", or "Kicked"
# Agent must write response for Task/Heartbeat, exit on Kicked

# Opt-in for dumb scripts: CLI handles heartbeats automatically
task=$(agent_pool get_task --pool $POOL --name $NAME --auto-heartbeat=true)
# task.kind is always "Task"
```

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
- `complete_task` sends response over socket
- No temp files, no directory watching
- Simpler, faster, easier to reason about

The CLI commands (`register`, `get_task`, `complete_task`) already abstract away the filesystem, so this would be a transparent change for agents using the CLI.

---

## CI Step Time Limits

Add a 5-minute timeout to each step in CI workflows. This prevents hung builds from consuming resources and provides faster feedback when something is stuck.

---

## Parallelize Pre-Commit Hook Checks

The pre-commit hook in `.githooks/pre-commit` runs checks sequentially:
1. `cargo fmt`
2. `cargo check`
3. `cargo clippy`
4. `cargo test`
5. `cargo udeps`

These could potentially run in parallel, especially when code is already compiled. Ideas:
- Run fmt first (modifies files), then run check/clippy/test/udeps in parallel
- Use `&` and `wait` in bash, or a tool like `parallel`
- If any fails, collect all errors before reporting

This would speed up the commit workflow, especially on incremental changes where compilation is cached.

---

## Agent CLI: Respond-and-Deregister / Abort Support

Add cleaner ways to stop agents that are more explicit about intent:

**`deregister_agent --data <response>`**: Allow deregistering while submitting a final response. Currently agents must call `next_task --data <response>` then `deregister_agent` separately.

```bash
# Current (two commands):
agent_pool next_task --pool $POOL --name $NAME --data '{"result": "done"}'
agent_pool deregister_agent --pool $POOL --name $NAME

# Proposed (one command):
agent_pool deregister_agent --pool $POOL --name $NAME --data '{"result": "done"}'
```

**`register --abort` / `next_task --abort`**: Allow agents to abort without providing a response. Useful when the agent can't complete the task and wants to let another agent try.

```bash
# Current: agent must provide some response
agent_pool next_task --pool $POOL --name $NAME --data '{"error": "abort"}'

# Proposed: explicit abort
agent_pool next_task --pool $POOL --name $NAME --abort
```

This also applies to `register` (abort the first task if the agent realizes it can't handle it).

Implementation notes:
- `--data` and `--abort` are mutually exclusive
- `--abort` without `--data` signals the daemon to requeue the task (or mark it failed)
- Need to decide: does abort requeue for another agent, or fail the task?

---

## Rethink Agent Deregistration

**Status: NEEDS THOUGHT**

The current `deregister_agent` behavior (write Kicked message, wait 50ms, remove directory) is problematic:

1. **Agents might be mid-task**: An agent CLI might be processing a task (not waiting on `next_task`) when we deregister. The Kicked message goes to `task.json`, but the CLI doesn't read `task.json` until it calls `next_task`. The agent completes its work, tries to respond, and finds its directory is gone.

2. **Race conditions in tests**: TestAgent.stop() calls `deregister_agent`, but the agent thread might be processing a task at that moment. The 50ms sleep is a hack that doesn't guarantee the CLI sees the Kicked message.

3. **No synchronization**: There's no way for `deregister_agent` to wait until the agent has actually stopped. It just removes the directory and hopes for the best.

Questions to answer:
- Should deregistration be synchronous (block until agent confirms it's stopping)?
- Should agents have a "stopping" state where they finish current task but don't accept new ones?
- Should there be a graceful vs forceful deregistration distinction?
- What happens to in-flight tasks when an agent is deregistered mid-task?

Possible approaches:
1. **Agent-initiated deregistration only**: Agents call `deregister_agent` on themselves when they want to stop. External deregistration is considered forceful/unsafe.

2. **Two-phase deregistration**: First write Kicked, then wait for agent to acknowledge by removing its own directory or writing a "goodbye" file.

3. **Daemon tracks agent state**: Daemon knows if agent is idle vs busy. Deregistration only succeeds on idle agents; busy agents get queued for deregistration after current task.

This needs careful design before implementing.

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
