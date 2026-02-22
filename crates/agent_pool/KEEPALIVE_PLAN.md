# Keepalive Plan

Replace file-based heartbeats with task-based keepalives (ping-pong).

## Motivation

### Initial Keepalive

When an agent (like Claude) first connects, we send a dummy "ping" task that forces it through the full protocol:
1. Agent receives task
2. Agent writes response
3. Daemon confirms receipt

**Why this matters:** For AI agents that require human approval for tool use, the initial keepalive gets approval out of the way with a harmless dummy task. After that, subsequent real tasks have already been "approved" by the same pattern, so they don't block on human interaction.

### Periodic Keepalive

Periodically send ping tasks to verify agents are still alive and responsive. If an agent fails to respond within a timeout, mark it as dead and reassign its in-flight work.

## Current State (Heartbeats)

```rust
// Agent periodically touches a file
agent_pool heartbeat --pool X --name Y  // touches agents/<name>/heartbeat

// Daemon checks mtime of heartbeat file
fn check_heartbeat_timeouts(&mut self) {
    let mtime = fs::metadata(&heartbeat_path).and_then(|m| m.modified());
    if mtime is stale { /* mark task as failed */ }
}
```

**Problems:**
- Agent must remember to send heartbeats during long tasks
- Heartbeats only matter during task execution, not for initial connection
- Doesn't help with the approval problem

## Target State (Keepalives)

```rust
pub struct DaemonConfig {
    /// Send a ping task when agent first registers.
    /// Helps get tool-use approvals out of the way.
    /// Default: true
    pub initial_keepalive: bool,

    /// Send periodic ping tasks to check agent health.
    /// Default: true
    pub periodic_keepalive: bool,

    /// Interval between periodic keepalives.
    /// Default: 60 seconds
    pub keepalive_interval: Duration,

    /// How long to wait for keepalive response before marking agent dead.
    /// Default: 30 seconds
    pub keepalive_timeout: Duration,
}
```

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

Agents treat keepalives like any other task. The instructions tell them what to do. No special client-side code needed - just follow the instructions.

For the command-agent.sh, we'd add handling:
```bash
KIND=$(echo "$TASK_JSON" | jq -r '.content.task.kind')
if [ "$KIND" = "Keepalive" ]; then
    ID=$(echo "$TASK_JSON" | jq -r '.content.task.value.id')
    echo "{\"id\": \"$ID\"}" > "$RESPONSE_FILE"
    continue
fi
```

### Daemon Flow

**Initial keepalive (on agent registration):**
```
Agent registers
    │
    ▼
Daemon sends Keepalive task
    │
    ▼
Agent responds (or times out)
    │
    ├─ Success: Agent marked as available for real tasks
    │
    └─ Timeout: Agent removed from pool
```

**Periodic keepalive:**
```
Timer fires (every keepalive_interval)
    │
    ▼
For each idle agent:
    │
    ▼
Send Keepalive task
    │
    ▼
Wait for response (up to keepalive_timeout)
    │
    ├─ Success: Agent stays in pool
    │
    └─ Timeout: Agent removed from pool
```

## Implementation Tasks

### Task 1: Add Keepalive task kind to protocol

**Files:** `crates/agent_pool/AGENT_PROTOCOL.md`

Add documentation about Keepalive tasks. No code changes.

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
    // Keep existing heartbeat_timeout for backward compat (deprecated)
    pub heartbeat_timeout: Option<Duration>,
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
4. On timeout, remove agent

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

In the event loop, track last keepalive time per agent. When interval elapses for an idle agent, send a keepalive.

```rust
struct AgentState {
    status: AgentStatus,
    last_keepalive: Option<Instant>,
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

**Commit:** `feat(agent_pool): implement periodic keepalive checks`

---

### Task 6: Handle keepalive timeout

**Files:** `crates/agent_pool/src/daemon.rs`

When a keepalive task times out:
1. Log warning
2. Remove agent from pool
3. If agent had a real task in-flight, requeue it

```rust
fn handle_keepalive_timeout(&mut self, agent_id: &str) {
    warn!(agent_id, "keepalive timeout, removing agent");
    if let Some(agent) = self.agents.remove(agent_id) {
        if let Some(in_flight) = agent.in_flight {
            // Requeue the task if it wasn't a keepalive
            if !in_flight.is_keepalive {
                self.pending.push_front(in_flight.task);
            }
        }
    }
}
```

**Commit:** `feat(agent_pool): handle keepalive timeout and requeue tasks`

---

### Task 7: Update command-agent.sh to handle keepalives

**Files:** `crates/agent_pool/scripts/command-agent.sh`

Add special handling for Keepalive tasks:

```bash
KIND=$(echo "$TASK_JSON" | jq -r '.content.task.kind // empty')
if [ "$KIND" = "Keepalive" ]; then
    echo "[$NAME] Responding to keepalive" >&2
    ID=$(echo "$TASK_JSON" | jq -r '.content.task.value.id')
    echo "{\"id\": \"$ID\"}" > "$RESPONSE_FILE"
    continue
fi
```

**Commit:** `feat(agent_pool): handle keepalive tasks in command-agent.sh`

---

### Task 8: Deprecate heartbeat mechanism

**Files:**
- `crates/agent_pool/src/daemon.rs`
- `crates/agent_pool/src/main.rs`
- `crates/agent_pool/AGENT_PROTOCOL.md`

1. Mark `heartbeat_timeout` config as deprecated
2. Add deprecation warning in CLI
3. Update docs to recommend keepalives instead
4. Keep heartbeat code working for backward compatibility

**Commit:** `chore(agent_pool): deprecate heartbeat in favor of keepalive`

---

### Task 9: Add tests for keepalive behavior

**Files:** `crates/agent_pool/tests/keepalive.rs`

Test cases:
- Initial keepalive sent on registration
- Agent not available until keepalive response
- Periodic keepalive sent after interval
- Agent removed on keepalive timeout
- In-flight task requeued on timeout

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
| 7 | Update command-agent.sh | 1 |
| 8 | Deprecate heartbeat | 4, 5, 6 |
| 9 | Add tests | 4, 5, 6 |

Tasks 1, 2, 7 can be done in parallel. Tasks 4, 5 can be done in parallel after 2. The goal is small, atomic commits that each leave the system in a working state.
