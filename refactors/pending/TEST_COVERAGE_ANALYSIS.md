# Test Coverage Analysis

**Status:** Reference document. All 6 submission modes (DataSource × NotifyMethod) are tested. See "Missing Test Scenarios" section for gaps.

## Current Test Inventory

### greeting.rs
| Test | What It Tests |
|------|---------------|
| `greeting_casual_and_formal` | Custom processor function handles different input styles correctly |

### single_basic.rs
| Test | What It Tests |
|------|---------------|
| `single_agent_single_task` | Basic happy path: one agent, one task, success |

### single_agent_queue.rs
| Test | What It Tests |
|------|---------------|
| `single_agent_queues_multiple_tasks` | Single agent processes multiple tasks sequentially (queuing) |

### many_agents.rs
| Test | What It Tests |
|------|---------------|
| `multiple_agents_parallel_tasks` | Multiple agents with varying speeds process tasks in parallel |

### integration.rs
| Test | What It Tests |
|------|---------------|
| `basic_submit` | Basic submit/response flow works |
| `single_agent_multiple_tasks` | Sequential task submission to single agent |
| `multiple_agents_parallel` | Two agents process tasks in parallel |
| `agent_deregistration` | Agent stops, new agent picks up subsequent work |
| `tasks_queued_before_agents` | Tasks submitted before any agent registers are queued and processed |
| `rapid_task_burst` | 10 tasks submitted rapidly all complete |
| `identical_task_content` | Multiple tasks with identical content are handled separately |
| `agent_joins_mid_processing` | Second agent joining helps with queued tasks |
| `response_isolation` | Each submitter receives the correct response |

---

## Event Taxonomy

### Daemon Lifecycle Events

| Event | Description | Tested? |
|-------|-------------|---------|
| Daemon starts | Pool directory created, watchers initialized | Yes (implicitly) |
| Daemon stops (graceful) | SIGTERM, clean shutdown | No |
| Daemon stops (SIGKILL) | Abrupt termination | No |
| Daemon crashes | Panic or fatal error | No |
| Daemon restarts | Stop then start with same directory | No |
| Daemon directory deleted | Pool directory removed while running | No |

### Agent Lifecycle Events

| Event | Description | Tested? |
|-------|-------------|---------|
| Agent registers (new) | Fresh agent joins pool | Yes |
| Agent registers (same name) | Agent re-registers with existing name | No |
| Agent registers (name collision) | Two agents try same name simultaneously | No |
| Agent deregisters (graceful) | Agent calls deregister | Yes (via TestAgent::stop) |
| Agent disappears (killed) | Agent process dies without deregister | No |
| Agent crashes mid-task | Agent dies while processing | No |
| Agent directory deleted | agents/<name>/ removed externally | No |

### Agent Response Events

| Event | Description | Tested? |
|-------|-------------|---------|
| Agent responds to heartbeat | Healthy ping-pong | Yes (implicitly) |
| Agent ignores heartbeat | No response within timeout | No |
| Agent responds to task (success) | Normal completion | Yes |
| Agent responds with invalid JSON | Malformed response | No |
| Agent responds after timeout | Late response | No |
| Agent responds to wrong task | Response mismatch | No |
| Agent responds with empty string | Edge case response | No |
| Agent responds with huge payload | Large response data | No |

### Task Lifecycle Events

| Event | Description | Tested? |
|-------|-------------|---------|
| Task submitted (Inline/Socket) | CLI --data --notify socket | Yes |
| Task submitted (Inline/File) | CLI --data --notify file | Yes |
| Task submitted (Inline/Raw) | Direct write, Inline envelope | Yes |
| Task submitted (FileRef/Socket) | CLI --file --notify socket | Yes |
| Task submitted (FileRef/File) | CLI --file --notify file | Yes |
| Task submitted (FileRef/Raw) | Direct write, FileReference envelope | Yes |
| Task dispatched to agent | Daemon assigns task | Yes (implicitly) |
| Task completed | Agent responds, response written | Yes |
| Task times out | No response within timeout | No |
| Task cancelled/withdrawn | Submitter abandons task | No |
| Task file deleted before read | Race condition | No |
| Task with invalid JSON | Malformed task.json | No |
| Task with missing fields | Incomplete payload | No |
| Task with huge payload | Large input data | No |

### Concurrency Events

| Event | Description | Tested? |
|-------|-------------|---------|
| Multiple agents idle | Several agents waiting | Yes |
| All agents busy | Queue backs up | Yes (implicitly) |
| Agent joins mid-processing | Dynamic scaling | Yes |
| Agent leaves mid-processing | Task reassignment? | Partial |
| Rapid task burst | Many tasks at once | Yes |
| Multiple submitters | Concurrent submissions | Yes |
| Task/agent race | Submit vs register timing | Yes |

### System Events

| Event | Description | Tested? |
|-------|-------------|---------|
| Disk full | Can't write response | No |
| Permission denied | File access errors | No |
| Network filesystem | NFS/SMB latency | No |
| Symlink in path | Pool dir is symlink | No |

---

## Missing Test Scenarios

### High Priority

#### 1. Agent Timeout
**Scenario:** Agent is assigned a task but never responds.
**Expected:** Daemon times out the task, marks agent as failed, re-queues or fails task.
**Why important:** This is a common failure mode in production.

```
Setup: Start daemon with short task timeout (e.g., 1 second)
       Register agent that sleeps forever on task
       Submit task
Assert: Task fails with timeout error
        Agent is deregistered or marked unhealthy
```

#### 2. Agent Crash Mid-Task
**Scenario:** Agent process dies while processing a task.
**Expected:** Daemon detects agent death, fails in-flight task.
**Why important:** Agents can crash due to bugs, OOM, etc.

```
Setup: Register agent
       Submit task
       Kill agent process mid-processing
Assert: Task fails (not hangs forever)
        Subsequent tasks can be processed by new agent
```

#### 3. Agent Disappears (No Deregister)
**Scenario:** Agent vanishes without calling deregister.
**Expected:** Daemon detects via heartbeat timeout or directory watch.
**Why important:** Network partitions, machine failures.

```
Setup: Register agent
       Delete agent's directory
Assert: Daemon notices agent is gone
        New agent with same name can register
```

#### 4. Daemon Graceful Shutdown
**Scenario:** Daemon receives SIGTERM while tasks are in flight.
**Expected:** In-flight tasks complete or fail gracefully, agents notified.
**Why important:** Deployments, restarts.

```
Setup: Register agent
       Submit long-running task
       Send SIGTERM to daemon
Assert: Task completes or fails cleanly
        No zombie processes
```

### Medium Priority

#### 5. Heartbeat Failure
**Scenario:** Agent stops responding to heartbeats.
**Expected:** Daemon deregisters agent after idle timeout.
**Why important:** Detects stuck agents.

```
Setup: Register agent that ignores heartbeats
       Wait for idle timeout
Assert: Agent is deregistered
```

#### 6. Agent Re-registration (Same Name)
**Scenario:** Agent disconnects and reconnects with same name.
**Expected:** Old registration replaced, tasks handled correctly.
**Why important:** Agent restarts.

```
Setup: Register agent "foo"
       Deregister agent "foo"
       Register agent "foo" again
Assert: Works correctly, no duplicate handling
```

#### 7. Task Cancellation
**Scenario:** Submitter withdraws task before completion.
**Expected:** Task removed from queue or marked cancelled.
**Why important:** User cancellation, timeouts.

```
Setup: Submit task
       Cancel task before agent picks it up
Assert: Agent doesn't receive task
        Or: agent receives but response is discarded
```

#### 8. Invalid Task JSON
**Scenario:** Submitter writes malformed JSON to pending/.
**Expected:** Daemon rejects gracefully, doesn't crash.
**Why important:** Robustness.

```
Setup: Write garbage to pending/<uuid>/task.json
Assert: Daemon logs error, continues running
        No crash or hang
```

#### 9. Invalid Agent Response
**Scenario:** Agent writes malformed JSON as response.
**Expected:** Daemon handles gracefully, reports error to submitter.
**Why important:** Robustness.

```
Setup: Register agent that responds with "not json"
       Submit task
Assert: Submitter receives error response
        Daemon continues running
```

### Low Priority

#### 10. Large Payloads
**Scenario:** Very large task input or response.
**Expected:** Handled correctly, no truncation.
**Why important:** Edge case, memory limits.

#### 11. Daemon Restart
**Scenario:** Daemon stops and starts with existing pending tasks.
**Expected:** Pending tasks are recovered and processed.
**Why important:** Crash recovery.

#### 12. Concurrent Agent Registration
**Scenario:** Two agents try to register with same name simultaneously.
**Expected:** One wins, other gets error or different name.
**Why important:** Race condition.

#### 13. File Descriptor Limits
**Scenario:** Many agents/tasks exhaust file descriptors.
**Expected:** Graceful degradation or clear error.
**Why important:** Production at scale.

---

## Test Matrix Summary

|  | Tested | Missing |
|--|--------|---------|
| Happy paths | ✓ | - |
| Agent lifecycle | Partial | Crash, timeout, disappear |
| Task lifecycle | Partial | Timeout, cancel, invalid |
| Daemon lifecycle | No | Shutdown, restart, crash |
| Error handling | No | Invalid JSON, permissions |
| Edge cases | Partial | Large payloads, limits |

---

## Implementation Notes

### Test Infrastructure Needed

1. **Controllable agents** - Agents that can:
   - Ignore heartbeats
   - Sleep forever
   - Crash on demand
   - Respond with specific data

2. **Daemon control** - Ability to:
   - Set short timeouts
   - Send signals
   - Monitor internal state

3. **Timing control** - Ability to:
   - Fast-forward time (or use short timeouts)
   - Control ordering of events
