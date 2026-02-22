# GSD (Get Sh*** Done)

A set of libraries and binaries for defining task queues managed by pools of agents.

## What is this?

GSD provides two complementary systems for parallel task processing:

### 1. Task Queue (`crates/task_queue`)

A Rust library for defining task queues as type-safe state machines. Tasks execute arbitrary shell scripts and deserialize their stdout.

**Interfaces:**
- **Rust API** - Define tasks with compile-time type safety, state machine semantics, and automatic task chaining
- **Binary API** *(planned)* - Submit tasks via JSON for use from any language

See [crates/task_queue/README.md](crates/task_queue/README.md) for API documentation.

### 2. Agent Pool (`crates/agent_pool`)

A daemon that manages a pool of long-running agents. Tasks are dispatched to available agents via a file-based protocol, enabling persistent workers that don't pay startup costs per task.

```bash
# Start the daemon
agent_pool start /path/to/root

# Submit a task (blocks until complete)
agent_pool submit /path/to/root "task input"

# Stop the daemon
agent_pool stop /path/to/root
```

## Example Use Cases

### Code Analysis and Refactoring Pipeline

A queue with two task types that form a pipeline:

1. **AnalyzeFile** - An agent analyzes a source file, identifying potential refactors
2. **PerformRefactor** - An agent executes a specific refactor

The workflow:
- Seed the queue with `AnalyzeFile` tasks for each source file
- Analysis agents process files and emit `PerformRefactor` tasks back to the queue
- Refactor agents pick up those tasks and apply changes
- The queue drains when all analysis is complete and all refactors are applied

### Invariant Enforcement

A self-healing linter that finds and fixes violations:

1. **Seed** - Find all `invariant.md` files in a codebase. Each describes (in English) invariants that must hold for that folder.

2. **ValidateInvariant** - An agent checks if a folder satisfies its invariants. On violation, it emits `QuickFix` tasks.

3. **QuickFix** - An agent applies a fix. When the last fix for a folder completes, re-queue `ValidateInvariant` for that folder.

4. **Retry limit** - Each `ValidateInvariant` tracks attempt count in context. After 3 failures, emit a catastrophic error instead of retrying.

```
Context {
    attempts: HashMap<PathBuf, u32>,      // folder -> attempt count
    pending_fixes: HashMap<PathBuf, u32>, // folder -> remaining fixes
    catastrophic_errors: Vec<PathBuf>,    // folders that couldn't be fixed
}
```

Setting `max_attempts = 1` turns this into a pure linter (validate only, no fixes).

### 3. GSD Runner (`crates/gsd`)

A high-level JSON-based orchestrator that sits on top of agent_pool. Define state machines via JSON config with JSON Schema validation.

```bash
# Run a state machine
gsd run config.json --root /tmp/pool --initial '[{"kind": "Start", "value": {}}]'

# Validate a config file
gsd validate config.json

# Generate documentation
gsd docs config.json
```

See [crates/gsd/DESIGN.md](crates/gsd/DESIGN.md) for the config format and protocol.

## Future Work

See [FUTURE.md](FUTURE.md) for the full roadmap.
