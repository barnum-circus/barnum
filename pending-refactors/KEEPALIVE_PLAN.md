# Keepalive Plan

Task-based keepalives (ping-pong) to verify agent health and pre-approve tool use.

## Motivation

### Initial Keepalive

When an agent (like Claude) first connects, we send a dummy "ping" task that forces it through the full protocol:
1. Agent receives task
2. Agent writes response
3. Daemon confirms receipt

**Why this matters:** For AI agents that require human approval for tool use, the initial keepalive gets approval out of the way with a harmless dummy task. After that, subsequent real tasks follow the same pattern, so they don't block on human interaction.

### Periodic Keepalive

Periodically send ping tasks to **idle** agents to verify they're still alive and responsive. This only happens when an agent has no task in progress - the purpose is to:
1. Keep agents from sleeping/timing out
2. Detect agents that have disconnected

If an agent fails to respond within the timeout, we deregister it from the pool.

## Configuration

```rust
pub struct DaemonConfig {
    /// Send a ping task when agent first registers.
    /// Helps get tool-use approvals out of the way.
    /// Default: true
    pub initial_keepalive: bool,

    /// Send periodic ping tasks to idle agents.
    /// Default: true
    pub periodic_keepalive: bool,

    /// Interval between periodic keepalives for idle agents.
    /// Default: 60 seconds
    pub keepalive_interval: Duration,

    /// How long to wait for keepalive response before deregistering agent.
    /// Default: 30 seconds
    pub keepalive_timeout: Duration,
}
```

## Protocol

### Ping Task Format

A keepalive is just a regular task with a special marker:

```json
{
    "task": {
        "kind": "Keepalive",
        "value": { "id": "ping-12345" }
    },
    "instructions": "Respond with the exact same id to confirm you are alive."
}
```

Expected response:
```json
{ "id": "ping-12345" }
```

### Agent Handling

Agents treat keepalives like any other task. The instructions tell them what to do. No special client-side code needed beyond following the instructions.

## Daemon Flow

### Initial Keepalive (on registration)

```
Agent registers (creates directory)
    │
    ▼
Daemon detects new agent
    │
    ▼
Daemon sends Keepalive task
    │
    ▼
Agent responds (or times out)
    │
    ├─ Success: Agent marked as available for real tasks
    │
    └─ Timeout: Agent directory removed (deregistered)
```

### Periodic Keepalive (idle agents only)

```
Timer fires (every keepalive_interval)
    │
    ▼
For each idle agent (no in-flight task):
    │
    ▼
Send Keepalive task
    │
    ▼
Wait for response (up to keepalive_timeout)
    │
    ├─ Success: Agent stays in pool, timer resets
    │
    └─ Timeout: Agent directory removed (deregistered)
```

**Key point:** Periodic keepalives only go to idle agents. There is never in-flight work to worry about during a keepalive timeout - we simply deregister the unresponsive agent.

### Recovery After Timeout

When an agent times out and is deregistered, it's treated exactly like a normal deregistration:

```
Agent times out on keepalive
    │
    ▼
Daemon removes agent directory
    │
    ▼
Agent is no longer in the pool
    │
    ▼
If agent is still alive and calls get_task:
    │
    ▼
get_task creates agent directory (re-registers)
    │
    ▼
Daemon detects new agent, sends initial keepalive
    │
    ▼
Agent responds, becomes available again
```

This means keepalive timeout is not a permanent failure - it's just a temporary removal. If the agent was merely slow (not dead), it can recover by simply calling `get_task` again. The re-registration triggers a fresh initial keepalive, and the agent rejoins the pool.

**Why this matters:** AI agents might occasionally be slow due to rate limits, user interaction delays, or other transient issues. Automatic recovery means the system is resilient - agents that come back online seamlessly rejoin without manual intervention.

## Implementation Tasks

### Task 1: Add Keepalive task kind to protocol

**Files:** `crates/agent_pool/AGENT_PROTOCOL.md`

Document the Keepalive task kind and expected response format.

**Commit:** `docs: document Keepalive task kind in agent protocol`

---

### Task 2: Add keepalive config options

**Files:** `crates/agent_pool/src/daemon.rs`

```rust
pub struct DaemonConfig {
    pub initial_keepalive: bool,      // default: true
    pub periodic_keepalive: bool,     // default: true
    pub keepalive_interval: Duration, // default: 60s
    pub keepalive_timeout: Duration,  // default: 30s
}
```

**Commit:** `feat(agent_pool): add keepalive config options`

---

### Task 3: Add CLI flags for keepalive config

**Files:** `crates/agent_pool/src/main.rs`

```rust
Command::Start {
    #[arg(long, default_value = "true")]
    initial_keepalive: bool,
    #[arg(long, default_value = "true")]
    periodic_keepalive: bool,
    #[arg(long, default_value = "60")]
    keepalive_interval_secs: u64,
    #[arg(long, default_value = "30")]
    keepalive_timeout_secs: u64,
}
```

**Commit:** `feat(agent_pool): add keepalive CLI flags to start command`

---

### Task 4: Implement initial keepalive on registration

**Files:** `crates/agent_pool/src/daemon.rs`

When an agent registers:
1. If `initial_keepalive` is true, mark agent as "pending_keepalive"
2. Dispatch a Keepalive task immediately
3. On response, mark agent as "available"
4. On timeout, remove agent directory

```rust
fn register(&mut self, agent_id: &str) {
    if self.config.initial_keepalive {
        self.agents.insert(agent_id.to_string(), AgentState::pending_keepalive());
        self.dispatch_keepalive(agent_id);
    } else {
        self.agents.insert(agent_id.to_string(), AgentState::available());
    }
}
```

**Commit:** `feat(agent_pool): send initial keepalive on agent registration`

---

### Task 5: Implement periodic keepalive

**Files:** `crates/agent_pool/src/daemon.rs`

Track last activity time per agent. When interval elapses for an idle agent, send a keepalive.

```rust
struct AgentState {
    status: AgentStatus,
    last_activity: Instant,
    in_flight: Option<InFlightTask>,
}

fn check_periodic_keepalives(&mut self) {
    if !self.config.periodic_keepalive {
        return;
    }
    for (id, agent) in &mut self.agents {
        if agent.is_idle() && agent.needs_keepalive(self.config.keepalive_interval) {
            self.dispatch_keepalive(id);
        }
    }
}
```

**Commit:** `feat(agent_pool): implement periodic keepalive for idle agents`

---

### Task 6: Handle keepalive timeout

**Files:** `crates/agent_pool/src/daemon.rs`

When a keepalive task times out:
1. Log warning
2. Remove agent directory (deregister)
3. Remove agent from internal state

Since keepalives only happen to idle agents, there's no in-flight work to handle.

```rust
fn handle_keepalive_timeout(&mut self, agent_id: &str) {
    warn!(agent_id, "keepalive timeout, deregistering agent");

    // Remove the agent directory
    let agent_dir = self.agents_dir.join(agent_id);
    let _ = fs::remove_dir_all(&agent_dir);

    // Remove from internal state
    self.agents.remove(agent_id);
}
```

**Commit:** `feat(agent_pool): deregister agent on keepalive timeout`

---

### Task 7: Update scripts to handle keepalives

**Files:**
- `crates/agent_pool/scripts/command-agent.sh`
- `crates/agent_pool/scripts/echo-agent.sh` (if exists)

Scripts that act as agents must handle Keepalive tasks:

```bash
KIND=$(echo "$TASK_JSON" | jq -r '.content.task.kind // empty')
if [ "$KIND" = "Keepalive" ]; then
    echo "[$NAME] Responding to keepalive" >&2
    ID=$(echo "$TASK_JSON" | jq -r '.content.task.value.id')
    echo "{\"id\": \"$ID\"}" > "$RESPONSE_FILE"
    continue
fi
```

**Commit:** `feat(agent_pool): handle keepalive tasks in shell agent scripts`

---

### Task 8: Update demos to handle keepalives

**Files:**
- `crates/agent_pool/demos/*.sh`

Demo scripts that spawn fake agents must either:
1. Handle Keepalive tasks (respond with the ping id), or
2. Start the daemon with `--initial-keepalive=false --periodic-keepalive=false`

For simple demos, disabling keepalives is cleaner:

```bash
agent_pool start --pool "$POOL" --initial-keepalive=false --periodic-keepalive=false &
```

For demos that test the full protocol, agents should handle keepalives:

```bash
# In the fake agent loop
if echo "$TASK" | jq -e '.content.task.kind == "Keepalive"' > /dev/null 2>&1; then
    ID=$(echo "$TASK" | jq -r '.content.task.value.id')
    echo "{\"id\": \"$ID\"}" > "$RESPONSE_FILE"
    continue
fi
```

**Commit:** `feat(agent_pool): update demos to handle or disable keepalives`

---

### Task 9: Add tests for keepalive behavior

**Files:** `crates/agent_pool/tests/keepalive.rs`

Test cases:
- Initial keepalive sent on registration (when enabled)
- Agent not available for real tasks until keepalive response
- Periodic keepalive sent after interval (to idle agents only)
- Agent deregistered on keepalive timeout
- Agent can re-register after timeout by calling get_task
- Keepalives disabled via config

**Commit:** `test(agent_pool): add keepalive behavior tests`

---

## Summary

| Task | Description | Depends On |
|------|-------------|------------|
| 1 | Document Keepalive in protocol | - |
| 2 | Add config options | - |
| 3 | Add CLI flags | 2 |
| 4 | Initial keepalive | 2 |
| 5 | Periodic keepalive | 2 |
| 6 | Timeout handling | 4, 5 |
| 7 | Update shell scripts | 1 |
| 8 | Update demos | 3, 7 |
| 9 | Add tests | 4, 5, 6 |

Tasks 1, 2, 7 can be done in parallel. Tasks 4, 5 can be done in parallel after 2.
