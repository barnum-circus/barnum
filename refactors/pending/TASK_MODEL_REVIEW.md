# Task Model Review

**Status:** Explored, decision made (2026-03-08)

**Blocks:** Nothing for 2.0.0. Sequence model is post-release.

**Note:** FINALLY_TRACKING completed 2026-03-08. FINALLY_SCHEDULING deferred - current shell-command finally acceptable for 2.0.0.

## Purpose

Step back and evaluate whether "main task + optional finally hook" is the right primitive. Consider alternatives before committing to the finally refactors.

## Current Model

A task is:
- Step name
- Input value
- Optional finally hook (shell command that runs when all descendants complete)

```
Task A (with finally)
├── spawns B, C
│   └── B spawns D
└── when B, C, D all done → run finally hook
```

Finally is bolted on - a separate concept attached to tasks.

## Questions to Answer

1. **Is "finally" the right abstraction?**
   - It's really "run this when my entire subtree completes"
   - Is this better modeled as a separate task type? A workflow primitive?

2. **What are the actual use cases for finally?**
   - Cleanup after fan-out
   - Aggregation/summary after parallel work
   - Notification that a phase completed
   - Something else?

3. **Are there patterns we're missing?**
   - Fan-out/fan-in (map-reduce style)
   - Sagas / compensating transactions
   - Checkpoints / milestones
   - Barriers (wait for N tasks before continuing)

4. **How do other systems model this?**
   - **Buck/Bazel**: Targets with deps, actions run when deps ready. No "finally" - just dependencies.
   - Temporal: activities, child workflows, signals
   - Airflow: task groups, trigger rules
   - Prefect: task dependencies, mapped tasks
   - Step Functions: parallel states, choice states

## Barnum as "Buck for Agents"

Conceptually, Barnum is Buck/Bazel for AI agents:
- **Buck**: targets → actions → artifacts, DAG of dependencies
- **Barnum**: tasks → agents → spawned tasks, dynamic DAG

Key difference: Buck's DAG is known upfront. Barnum's DAG is dynamic - tasks spawn new tasks at runtime.

### Buck Model

```
target A depends on B, C
B depends on D
# Build order: D → B, C (parallel) → A
```

- Dependencies declared upfront
- Execution order derived from deps
- No "finally" - if A depends on B, A runs after B

### Barnum Model

```
task A runs, spawns B, C at runtime
B runs, spawns D at runtime
# A doesn't know about D until B runs
```

- Dependencies discovered at runtime (agent decides what to spawn)
- "Finally" is: run something after dynamic subtree completes

## Build System Concepts à la Carte

What primitives do build systems have? Which translate to Barnum?

| Concept | Buck/Bazel | Barnum Equivalent | Notes |
|---------|-----------|----------------|-------|
| **Target** | Named build unit | Step | Static vs dynamic |
| **Rule** | How to build a target | Step config + action | Similar |
| **Action** | Actual work (compile, link) | Agent task | Agents instead of tools |
| **Deps** | Explicit dependencies | `origin_id` (implicit) | Barnum deps are runtime-discovered |
| **Provider** | Data passed between targets | Task value / response | Similar |
| **Depset** | Accumulated deps | N/A | Could be useful for fan-in? |
| **Configuration** | Build flavor (debug/release) | N/A | Not needed? |
| **Transition** | Change config mid-graph | N/A | |
| **Aspect** | Cross-cutting concern | N/A | Could be useful? |
| **Genrule** | Arbitrary shell command | Command action | Same |

### Concepts Barnum Is Missing?

**Depset / Accumulation**
- Buck: depsets accumulate values up the tree
- Barnum currently: no equivalent. Finally gets parent's value, not children's results
- After FINALLY_TRACKING refactor: tree-based tracking enables collecting child results
- Could pass to after task: `{ parent_value: ..., child_results: [...] }`

**Explicit Dependencies**
- Buck: target declares what it depends on
- Barnum: deps discovered at runtime (agent spawns tasks)
- Trade-off: flexibility vs predictability

**Providers / Typed Data Flow**
- Buck: providers define what data flows between targets
- Barnum: just JSON values
- Could add: typed schemas for inter-task data

**Build Graph Analysis**
- Buck: can query/analyze graph before building
- Barnum: graph unknown until runtime
- Fundamental difference, probably can't change

### Insight

In Buck, you'd model "finally" as a target that depends on the fan-out targets:

```python
# Buck
target(name="process_all", deps=[":process_1", ":process_2", ":process_3"])
target(name="aggregate", deps=[":process_all"])  # runs after all process_* done
```

But Barnum can't do this because we don't know the spawned tasks upfront.

### Possible Direction

What if tasks could declare "synthetic dependencies"?

```
Task A spawns B, C with: "when done, spawn Aggregate with all results"
```

This is basically fan-in as a first-class concept, not "finally".

## Alternative Models

### A. Explicit Fan-In Task

Instead of "finally hook", have explicit fan-in:

```json
{
  "steps": {
    "Process": {
      "action": "...",
      "fan_in": "Aggregate"  // when all Process tasks done, spawn one Aggregate
    },
    "Aggregate": {
      "action": "..."
    }
  }
}
```

Pro: Cleaner model, fan-in is a first-class step
Con: How do you get results from all Process tasks into Aggregate?

### B. Task Groups / Scopes

Tasks can spawn a "group" that has its own completion semantics:

```rust
enum TaskSpawn {
    // Regular child task
    Task(Task),
    // Group of tasks with completion handler
    Group {
        tasks: Vec<Task>,
        on_complete: Task,  // runs when all tasks in group done
    },
}
```

Pro: Explicit grouping, completion handler is just another task
Con: More complex spawning model

### C. Workflow as First-Class

Separate "workflow" from "task":
- Task: single unit of work, stateless
- Workflow: orchestrates tasks, has state, handles fan-out/fan-in

```json
{
  "workflows": {
    "ProcessAll": {
      "fan_out": { "step": "Process", "for_each": "$.items" },
      "fan_in": { "step": "Aggregate", "collect": "$.results" }
    }
  }
}
```

Pro: Clear separation of concerns
Con: Two concepts instead of one, more complex

### D. Keep Finally, But Make It a Task

Current direction: finally becomes a regular task with `finally_for` field.

Pro: Minimal model change, finally is "just a task"
Con: Still feels bolted-on, `finally_for` is a special case

## Evaluation Criteria

- **Simplicity**: How easy to understand?
- **Composability**: Can primitives combine naturally?
- **Persistence**: Can state be logged/reconstructed?
- **Flexibility**: Does it handle real use cases?
- **Implementation**: How much work to build?

## Recommendation

TBD after exploration. Current plan (finally as task) might be fine, or we might want something cleaner.

## Simpler Primitives?

### Option E: Two-Phase Return

Agent returns two lists:

```rust
struct TaskResult {
    spawned: Vec<Task>,  // run immediately
    after: Vec<Task>,    // run when all spawned (and their descendants) done
}
```

"Finally" becomes just the `after` list. No config hook - it's part of the response.

Pro:
- No special finally concept in config
- Agent controls continuation, not config
- Just tasks, no hooks

Con:
- Every response has two lists (usually `after` is empty)
- Agent decides finally logic, not config

### Option F: Barrier Task

Special task type that waits:

```rust
enum Task {
    Regular { step, value },
    Barrier { wait_for: Vec<TaskId>, then: Box<Task> },
}
```

Agent can spawn: "run these 3 tasks, then when done run this aggregate task".

Pro: Explicit dependencies, composable
Con: Agent needs to know task IDs, more complex

### Option G: Phases

Tasks have a phase number. Phase N+1 waits for all phase N to complete:

```json
[
    { "step": "Process", "value": {...}, "phase": 0 },
    { "step": "Process", "value": {...}, "phase": 0 },
    { "step": "Aggregate", "value": {}, "phase": 1 }
]
```

Pro: Simple, declarative
Con: Only works for one level of waiting

### Simplest Possible?

Maybe just:

```json
[
    { "step": "Process", "value": {...} },
    { "step": "Process", "value": {...} },
    { "step": "Aggregate", "value": {}, "after_siblings": true }
]
```

One flag: `after_siblings`. If true, waits for all sibling subtrees to complete.

This is "finally" but:
- Controlled by agent response, not config
- Just a flag on a regular task
- No special hooks or callbacks

## Option H: Sequence as Fundamental Primitive (2026-03-08)

**Key insight:** The real primitive is *sequence* - a chain of steps where each waits for the previous step's subtree to complete before running.

### Why Sequences?

The thread exhaustion problem: we can't block a thread waiting for children to complete. So continuation must be *declarative* - expressed as config data, not as a blocked call stack.

Current model has an implicit sequence per step:
```
pre_hook → action → post_hook → [children] → finally_hook
```

But this is really just a sequence with special names. Pre/post are sequence items that happen to be synchronous transformations. Finally is a sequence item that runs after children complete.

### Sequence Model

A step could have a `next` field pointing to another step:

```yaml
steps:
  - name: Analyze
    action: { pool: analyze }
    next: Aggregate  # after Analyze subtree completes, run Aggregate

  - name: Aggregate
    action: { command: "./aggregate.sh" }
    next: Notify

  - name: Notify
    action: { command: "./notify.sh" }
```

Or equivalently, an explicit sequence:

```yaml
steps:
  - name: ProcessAll
    sequence: [Analyze, Aggregate, Notify]
```

Both express: run Analyze (wait for subtree), then Aggregate (wait for subtree), then Notify.

### Value Flow

In a sequence A → B:
- B receives A's output (the effective value / result)
- B also receives a hashmap of results from A's children

This enables both:
- **Cleanup patterns**: B uses A's original value to know what to clean up
- **Aggregation patterns**: B uses children's results to compute summary

### Relationship to Current Model

| Current | Sequence Model |
|---------|----------------|
| `finally: "./cleanup.sh"` | `next: CleanupStep` |
| `pre_hook` | First item in implicit sequence (sync) |
| `post_hook` | Item after action in implicit sequence (sync) |

Pre/post are just sequence items that happen to be synchronous. The distinction between sync/async doesn't matter at the model level - it's an implementation detail.

### Config-Controlled, Not Agent-Controlled

**Critical constraint:** Sequences must be defined in config, not by agents. The agent runs a step and spawns children, but the config determines what continuation runs after. This prevents agents from arbitrarily extending execution and keeps the workflow predictable.

### Implementation Notes

The FINALLY_TRACKING refactor (completed 2026-03-08) provides the foundation:
- `BTreeMap<LogTaskId, TaskEntry>` with `parent_id` for tree structure
- `Continuation` type that holds "what to run when children complete"
- `TaskState::Waiting { pending_count, continuation }` for deferred execution

Changing from shell-command finally to step-reference `next` would:
1. Change `finally_hook: HookScript` to `next: Option<StepName>` in step config
2. Create a task for the next step instead of running a shell command
3. Pass (parent_output, children_results) as the next step's input value

### Timeline

This is a significant model change. **Not blocking 2.0.0 release.**

For 2.0.0: proceed with state persistence using current shell-command finally model. Accept that finally hooks have limited persistence (may re-run on resume).

Post-2.0.0: implement sequence model properly.

## Action Items

1. List concrete use cases for finally in real workflows
2. Sketch how each alternative handles those use cases
3. Evaluate against criteria
4. ~~Decide: proceed with finally refactors, or pivot to new model~~
5. **Decision (2026-03-08):** Proceed with current model for 2.0.0. Sequence model is post-release work.
