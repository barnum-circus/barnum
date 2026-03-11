# State persistence and resume

Long-running Barnum workflows can be interrupted by crashes, Ctrl+C, or OOM. State persistence lets you pick up where you left off.

## How it works

Every run writes an NDJSON (newline-delimited JSON) state log. Each line records one event: a task being submitted or a task completing. On resume, Barnum replays the log to figure out what's still pending, then continues from there.

```
Normal run:
  config → task 0 submitted → task 0 completed → task 1 submitted → ...
                                                                    ↑ crash

Resume:
  replay log → task 1 is pending → re-dispatch task 1 → continue
```

Completed tasks are never re-executed. Only pending and in-flight tasks are re-dispatched.

## CLI usage

```bash
# Normal run with state logging
barnum run --config config.jsonc --pool agents \
  --entrypoint-value '{"files": ["src/main.rs"]}' \
  --state-log /tmp/myrun.ndjson

# Resume from a previous run
barnum run --resume-from /tmp/myrun.ndjson \
  --state-log /tmp/myrun-resumed.ndjson
```

`--resume-from` is incompatible with `--config`, `--initial-state`, and `--entrypoint-value`, since the config is stored in the log itself.

The resume log must be a different file from the original (no in-place mutation).

## State log format

The log is NDJSON. The first entry is always the config. Every subsequent entry is either a task submission or a task completion.

```jsonc
// First entry: the full config (frozen at run start)
{"kind":"Config","config":{...}}

// Root task submitted
{"kind":"TaskSubmitted","task_id":0,"step":"Analyze","value":{"file":"src/main.rs"},"parent_id":null,"origin":"Initial"}

// Task completed, spawning two children
{"kind":"TaskCompleted","task_id":0,"outcome":{"kind":"Success","value":{"spawned_task_ids":[1,2]}}}

// Children submitted
{"kind":"TaskSubmitted","task_id":1,"step":"Process","value":{"data":"x"},"parent_id":0,"origin":"Spawned"}
{"kind":"TaskSubmitted","task_id":2,"step":"Process","value":{"data":"y"},"parent_id":0,"origin":"Spawned"}

// Child completes
{"kind":"TaskCompleted","task_id":1,"outcome":{"kind":"Success","value":{"spawned_task_ids":[]}}}

// Task fails, gets retried
{"kind":"TaskCompleted","task_id":2,"outcome":{"kind":"Failed","value":{"reason":{"kind":"Timeout"},"retry_task_id":3}}}
{"kind":"TaskSubmitted","task_id":3,"step":"Process","value":{"data":"y"},"parent_id":0,"origin":{"Retry":{"replaces":2}}}

// Finally hook fires after all children complete
{"kind":"TaskSubmitted","task_id":4,"step":"Analyze","value":{},"parent_id":null,"origin":{"Finally":{"finally_for":0}}}
```

### Task origins

Every submitted task records how it came to exist:

| Origin | Meaning |
|--------|---------|
| `Initial` | Root task from `--entrypoint-value` |
| `Spawned` | Created by a parent task's output |
| `Retry { replaces }` | Replacement for a failed task |
| `Finally { finally_for }` | Finally hook for a completed task |

### Completion outcomes

| Outcome | Fields |
|---------|--------|
| `Success` | `spawned_task_ids`: IDs of children this task created |
| `Failed` | `reason` (Timeout, AgentLost, InvalidResponse) and optional `retry_task_id` |

## Reconstruction algorithm

On resume, Barnum replays the log and classifies every task:

| Log state | Reconstructed as |
|-----------|-----------------|
| Submitted, never completed | **Pending**: re-dispatch the action |
| Completed successfully, some children still pending | **Waiting**: don't re-run, just wait for children |
| Completed successfully, all children done | **Done**: removed from state |
| Failed with `retry_task_id` | **Done**: the retry task handles it |
| Failed without retry | **Done**: task was dropped |

The algorithm:

1. Build a map of all submitted tasks and all completed tasks
2. Seed the "alive" set with all submitted-but-not-completed tasks
3. Propagate upward: if a completed task has any alive dependents, mark it alive too
4. Repeat until no new alive tasks are found (fixed point)
5. Classify alive tasks as pending (need dispatch) or waiting (need children to finish)

## Resume guarantees

**Completed tasks are never re-executed.** If a task appears in the log as completed, it won't be dispatched again.

**In-flight tasks are re-dispatched.** If the process crashed while a task was running, the task was submitted but never completed, so it gets re-dispatched with its original input. This may cause duplicate work for that specific task, but it's safe.

**Task IDs are preserved.** IDs from the original run are never reused. New tasks get IDs continuing from where the original run left off. This keeps parent-child references valid.

**Config is frozen.** The config stored in the log is used on resume, not a fresh read of the config file. This prevents mid-run config drift.

**Retry counts are reconstructed.** The number of retries consumed is computed by following the `Retry { replaces }` chain in the log, then compared against `max_retries` from the config.

**Finally hooks are preserved.** When a task completes with children, its input value is stored in the log. On resume, if children are still pending, the finally hook can fire with the correct value when they eventually complete.

## Example: crash and resume

Consider a fan-out workflow: ListFiles → Refactor (per file) → finally commit.

```
ListFiles (command) → fans out to 3 Refactor tasks
  Refactor file-a.js ✓ completed
  Refactor file-b.js   in-flight (crash!)
  Refactor file-c.js   pending
```

The state log at crash time:

```jsonc
{"kind":"Config","config":{...}}
{"kind":"TaskSubmitted","task_id":0,"step":"ListFiles","value":{},"parent_id":null,"origin":"Initial"}
{"kind":"TaskCompleted","task_id":0,"outcome":{"kind":"Success","value":{"spawned_task_ids":[1,2,3]}}}
{"kind":"TaskSubmitted","task_id":1,"step":"Refactor","value":{"file":"file-a.js"},"parent_id":0,"origin":"Spawned"}
{"kind":"TaskSubmitted","task_id":2,"step":"Refactor","value":{"file":"file-b.js"},"parent_id":0,"origin":"Spawned"}
{"kind":"TaskSubmitted","task_id":3,"step":"Refactor","value":{"file":"file-c.js"},"parent_id":0,"origin":"Spawned"}
{"kind":"TaskCompleted","task_id":1,"outcome":{"kind":"Success","value":{"spawned_task_ids":[]}}}
```

On resume, reconstruction finds:
- Task 0 (ListFiles): completed, but children 2 and 3 are alive → **waiting**
- Task 1 (Refactor file-a.js): completed, no alive dependents → **done**
- Task 2 (Refactor file-b.js): submitted, not completed → **pending** (re-dispatch)
- Task 3 (Refactor file-c.js): submitted, not completed → **pending** (re-dispatch)

Barnum re-dispatches tasks 2 and 3. When both complete, task 0's finally hook fires. Task 1 (file-a.js) is not touched again.

## Key points

- State log is NDJSON, one JSON object per line, flushed after each write
- Config is stored in the log and frozen for the lifetime of the run
- Resume reads the old log, reconstructs state, then creates a new log (copies old entries + appends new ones)
- Completed tasks are skipped; pending and in-flight tasks are re-dispatched
- Task IDs are monotonic and preserved across resume
- `--resume-from` replaces `--config` and `--entrypoint-value`, since everything comes from the log
