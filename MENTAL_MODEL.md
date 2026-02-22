# Mental Model

## Task Queue

The task queue executes arbitrary shell scripts and deserializes their stdout. It is agnostic to what the script does - it could be a simple bash command, a Python script, or an invocation of `agent_pool submit` to dispatch work to a pool of persistent agents.

### Key Structs

#### 1. Queue Item
The item in the queue. Implements `QueueItem<Context>` trait directly.

- Generic over context type
- Associated types:
  - `InProgress` - the in-progress state for this item
  - `Response` - deserialized from script stdout
  - `NextTasks: Into<Vec<Task>>` - constrains which tasks this item can transition to (enables state machine patterns)
- `fn start(self, ctx) -> (Self::InProgress, Command)`:
  - Consumes the queue item and mutable context
  - Returns in-progress state AND a command to run
- `fn cleanup(in_progress, result, ctx) -> Self::NextTasks`:
  - Called after script completes
  - Returns tasks to add back to the queue

#### 2. Task Enum
The global task enum. Each variant wraps a queue item. Derives `GsdTask`, which generates dispatch logic to the underlying item's `QueueItem` impl.

#### 3. Response Struct
Deserialized from the script's stdout.

### Context

Mutable state passed through the entire processing pipeline. Passed to both `start` and `cleanup`.

### Overall Flow

1. Initialize `queue` with initial items, `in_flight` as empty
2. While `queue` is not empty OR `in_flight` is not empty:
   1. While `in_flight.len() < max_concurrency` AND `queue` is not empty:
      1. Pop item from queue
      2. Call `item.start(ctx)` (synchronous)
      3. Spawn script, add `(in_progress, handle)` to `in_flight`
   2. Wait for any script in `in_flight` to complete
   3. Deserialize stdout into response
   4. Call `cleanup(in_progress, response, ctx)` (synchronous)
   5. Add returned tasks to queue

Only script execution is async/parallel. Everything else (`start`, `cleanup`, queue mutations) is synchronous.

### Configuration

- `max_concurrency: Option<usize>` - `None` means unbounded

---

## Agent Pool

The agent pool (`agent_pool`) manages a pool of persistent worker agents.

### CLI Commands

```bash
# Start the daemon
agent_pool start <root>

# Submit a task and wait for result
agent_pool submit <root> <input>

# Stop the daemon
agent_pool stop <root>
```

### Folder Structure

```
<root>/
├── daemon.lock      # Lock file (PID of running daemon)
├── daemon.sock      # Unix socket for IPC
└── agents/
    └── {agent_id}/  # Each agent has its own folder
        ├── next_task     # Written by daemon to assign work
        ├── in_progress   # Agent renames next_task here (atomic claim)
        └── output        # Written by agent when complete
```

### Protocol

**Task Submission (via socket):**
1. Client connects to `daemon.sock`
2. Client sends: `{length}\n{content}`
3. Daemon queues task, waits for available agent
4. Daemon writes `agents/{agent_id}/next_task`
5. Agent claims task via atomic rename: `next_task → in_progress`
6. Agent processes, writes `output`
7. Daemon reads output, sends to client: `{length}\n{content}`
8. Daemon cleans up task files

**Agent Loop:**
1. Watch for `next_task` file
2. Atomically rename to `in_progress` (claim the task)
3. Read content, process it
4. Write result to `output`
5. Delete `in_progress`
6. Repeat

### Agent Lifecycle

- One task at a time per agent
- Atomic rename prevents race conditions between multiple agents
- Agent folder existence = agent registration

---

## GSD Runner

The GSD runner (`gsd`) is a high-level JSON-based orchestrator that sits on top of agent_pool.

### CLI Commands

```bash
# Run a state machine
gsd run <config> --root <pool-root> --initial <tasks>

# Validate a config file
gsd validate <config>

# Generate documentation
gsd docs <config>
```

### Config Format

```json
{
  "options": { "timeout": 120, "max_retries": 3 },
  "steps": [
    {
      "name": "Analyze",
      "value_schema": { "type": "object" },
      "action": { "kind": "Pool", "instructions": "Analyze the input." },
      "next": ["Implement", "Done"]
    },
    {
      "name": "Transform",
      "action": { "kind": "Command", "script": "jq '.value' | process.sh" },
      "next": ["Done"]
    }
  ]
}
```

### Action Types

- **pool**: Send to agent pool (default). Has `instructions` for the LLM agent.
- **command**: Run a local script. Task JSON on stdin, response array on stdout.

### Task Format

Tasks are JSON objects with `kind` and `value`:
```json
{"kind": "Analyze", "value": {"file": "main.rs"}}
```

Agent responses are arrays of tasks:
```json
[{"kind": "Implement", "value": {...}}]
```

### Agent Payload

GSD sends agents structured payloads:
```json
{
  "task": {"kind": "Analyze", "value": {...}},
  "instructions": "# Current Step: Analyze\n...",
  "timeout_seconds": 120
}
```

### Validation Flow

1. Initial task validated against step's schema
2. Agent response must be a JSON array
3. Each task's `kind` must be a valid `next` step
4. Each task's `value` validated against target step's schema
5. Invalid responses trigger retry (up to `max_retries`)
