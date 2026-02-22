# Health Check Plan

Task-based health checks (ping-pong) to verify agent health and pre-approve tool use.

## Motivation

### Initial Health Check

When an agent first connects, we send a health check that forces it through the full protocol. This gets tool-use approvals out of the way with a harmless dummy task.

### Periodic Health Check

Periodically send health checks to **idle** agents to verify they're still alive. If an agent fails to respond within the timeout, we deregister it. Agents can recover by calling `get_task` again.

## Current State

**AgentStatus enum** (already implemented):
```rust
// daemon.rs lines 256-268
enum AgentStatus {
    Idle,
    Busy(InFlight),
}

enum InFlight {
    Task { respond_to: ResponseTarget },
}
```

**DaemonConfig** (currently empty):
```rust
// daemon.rs lines 45-49
pub struct DaemonConfig {
    // Reserved for future configuration options (e.g., keepalive settings)
}
```

**get_task output** (always "Task"):
```rust
// main.rs lines 318-322
let output = serde_json::json!({
    "kind": "Task",
    "response_file": response_file.display().to_string(),
    "content": content_json
});
```

---

## Implementation Tasks

| Status | Task | Description |
|--------|------|-------------|
| [ ] | 1 | Add `InFlight::HealthCheck` variant |
| [ ] | 2 | Add health check config to `DaemonConfig` |
| [ ] | 3 | Add CLI flags for health check config |
| [ ] | 4 | Update `get_task` to handle `HealthCheck` kind |
| [ ] | 5 | Add `dispatch_health_check()` method |
| [ ] | 6 | Update `register()` to send initial health check |
| [ ] | 7 | Add `last_activity` tracking to `AgentState` |
| [ ] | 8 | Add periodic health check to event loop |
| [ ] | 9 | Handle health check timeout |
| [ ] | 10 | Update `complete_task()` to handle `HealthCheck` |
| [ ] | 11 | Update shell scripts |
| [ ] | 12 | Update demos |
| [ ] | 13 | Add tests |

---

### Task 1: Add `InFlight::HealthCheck` variant

**File:** `crates/agent_pool/src/daemon.rs` lines 264-268

**Before:**
```rust
enum InFlight {
    /// A real task from a submitter.
    Task { respond_to: ResponseTarget },
}
```

**After:**
```rust
enum InFlight {
    /// A real task from a submitter.
    Task { respond_to: ResponseTarget },
    /// A health check (initial or periodic).
    HealthCheck,
}
```

---

### Task 2: Add health check config to `DaemonConfig`

**File:** `crates/agent_pool/src/daemon.rs` lines 45-49

**Before:**
```rust
pub struct DaemonConfig {
    // Reserved for future configuration options (e.g., keepalive settings)
}
```

**After:**
```rust
pub struct DaemonConfig {
    /// Send a health check when agent first registers.
    /// Default: true
    pub initial_health_check: bool,
    /// Send periodic health checks to idle agents.
    /// Default: true
    pub periodic_health_check: bool,
    /// Interval between periodic health checks.
    /// Default: 60 seconds
    pub health_check_interval: Duration,
    /// Timeout for health check response.
    /// Default: 30 seconds
    pub health_check_timeout: Duration,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            initial_health_check: true,
            periodic_health_check: true,
            health_check_interval: Duration::from_secs(60),
            health_check_timeout: Duration::from_secs(30),
        }
    }
}
```

**Also:** Remove the `#[derive(Default)]` from the struct since we're implementing it manually.

**Also:** Remove the `#[expect(dead_code)]` from `PoolState.config` (line 291-292).

---

### Task 3: Add CLI flags for health check config

**File:** `crates/agent_pool/src/main.rs` lines 50-61 (Command::Start)

**Before:**
```rust
Command::Start {
    #[arg(long)]
    pool: Option<String>,
    #[arg(short, long, default_value = "info")]
    log_level: LogLevel,
    #[arg(long)]
    json: bool,
},
```

**After:**
```rust
Command::Start {
    #[arg(long)]
    pool: Option<String>,
    #[arg(short, long, default_value = "info")]
    log_level: LogLevel,
    #[arg(long)]
    json: bool,
    #[arg(long, default_value = "true")]
    initial_health_check: bool,
    #[arg(long, default_value = "true")]
    periodic_health_check: bool,
    #[arg(long, default_value = "60")]
    health_check_interval_secs: u64,
    #[arg(long, default_value = "30")]
    health_check_timeout_secs: u64,
},
```

**Also:** Update the match arm (around line 146) to construct `DaemonConfig`:
```rust
let config = DaemonConfig {
    initial_health_check,
    periodic_health_check,
    health_check_interval: Duration::from_secs(health_check_interval_secs),
    health_check_timeout: Duration::from_secs(health_check_timeout_secs),
};
```

**Also:** Change `run(&root)` to `run_with_config(&root, config)`.

---

### Task 4: Update `get_task` to handle `HealthCheck` kind

**File:** `crates/agent_pool/src/main.rs` lines 318-322

The daemon will write a JSON object with `"kind": "HealthCheck"` or `"kind": "Task"` to task.json. The `get_task` command reads this and outputs it directly.

**Before:**
```rust
let output = serde_json::json!({
    "kind": "Task",
    "response_file": response_file.display().to_string(),
    "content": content_json
});
```

**After:**
```rust
// The daemon writes the kind to task.json, so just pass it through
let kind = content_json.get("kind")
    .and_then(|k| k.as_str())
    .unwrap_or("Task");

let output = serde_json::json!({
    "kind": kind,
    "response_file": response_file.display().to_string(),
    "content": content_json.get("content").cloned().unwrap_or(content_json.clone())
});
```

**Note:** This means the daemon must write task.json in a new format:
```json
{"kind": "Task", "content": {...original task...}}
{"kind": "HealthCheck", "content": {"instructions": "..."}}
```

---

### Task 5: Add `dispatch_health_check()` method

**File:** `crates/agent_pool/src/daemon.rs` (add after `dispatch_to()`, around line 495)

**Add:**
```rust
fn dispatch_health_check(&mut self, agent_id: &str) -> io::Result<()> {
    let Some(agent) = self.agents.get_mut(agent_id) else {
        return Err(io::Error::other("agent not found"));
    };

    let task_content = serde_json::json!({
        "kind": "HealthCheck",
        "content": {
            "instructions": "Respond with any value to confirm you are alive."
        }
    });

    let task_path = self.agents_dir.join(agent_id).join(TASK_FILE);
    debug!(agent_id, path = %task_path.display(), "writing health check");
    fs::write(&task_path, task_content.to_string())?;

    info!(agent_id, "health check dispatched");
    agent.status = AgentStatus::Busy(InFlight::HealthCheck);
    Ok(())
}
```

---

### Task 6: Update `register()` to send initial health check

**File:** `crates/agent_pool/src/daemon.rs` lines 422-427

**Before:**
```rust
fn register(&mut self, agent_id: &str) {
    if !self.agents.contains_key(agent_id) {
        info!(agent_id, "agent registered");
        self.agents.insert(agent_id.to_string(), AgentState::new());
    }
}
```

**After:**
```rust
fn register(&mut self, agent_id: &str) {
    if !self.agents.contains_key(agent_id) {
        info!(agent_id, "agent registered");
        self.agents.insert(agent_id.to_string(), AgentState::new());

        if self.config.initial_health_check {
            if let Err(e) = self.dispatch_health_check(agent_id) {
                warn!(agent_id, error = %e, "failed to dispatch initial health check");
            }
        }
    }
}
```

---

### Task 7: Add `last_activity` tracking to `AgentState`

**File:** `crates/agent_pool/src/daemon.rs` lines 270-283

**Before:**
```rust
struct AgentState {
    status: AgentStatus,
}

impl AgentState {
    const fn new() -> Self {
        Self { status: AgentStatus::Idle }
    }

    const fn is_idle(&self) -> bool {
        matches!(self.status, AgentStatus::Idle)
    }
}
```

**After:**
```rust
struct AgentState {
    status: AgentStatus,
    last_activity: Instant,
}

impl AgentState {
    fn new() -> Self {
        Self {
            status: AgentStatus::Idle,
            last_activity: Instant::now(),
        }
    }

    const fn is_idle(&self) -> bool {
        matches!(self.status, AgentStatus::Idle)
    }

    fn touch(&mut self) {
        self.last_activity = Instant::now();
    }
}
```

**Note:** Remove `const` from `new()` since `Instant::now()` is not const.

---

### Task 8: Add periodic health check to event loop

**File:** `crates/agent_pool/src/daemon.rs` in `event_loop()` (around line 580)

**Add method to PoolState:**
```rust
fn check_periodic_health_checks(&mut self) -> io::Result<()> {
    if !self.config.periodic_health_check {
        return Ok(());
    }

    let interval = self.config.health_check_interval;
    let needs_check: Vec<_> = self
        .agents
        .iter()
        .filter(|(_, a)| a.is_idle() && a.last_activity.elapsed() >= interval)
        .map(|(id, _)| id.clone())
        .collect();

    for agent_id in needs_check {
        if let Err(e) = self.dispatch_health_check(&agent_id) {
            warn!(agent_id, error = %e, "failed to dispatch periodic health check");
        }
    }

    Ok(())
}
```

**Update event loop** (add after `scan_pending()` call, around line 577):
```rust
state.check_periodic_health_checks()
    .map_err(|e| io::Error::new(e.kind(), format!("health check failed: {e}")))?;
```

---

### Task 9: Handle health check timeout

**File:** `crates/agent_pool/src/daemon.rs` (add method to PoolState)

**Add:**
```rust
fn check_health_check_timeouts(&mut self) -> io::Result<()> {
    let timeout = self.config.health_check_timeout;

    let timed_out: Vec<_> = self
        .agents
        .iter()
        .filter(|(_, a)| {
            matches!(a.status, AgentStatus::Busy(InFlight::HealthCheck))
                && a.last_activity.elapsed() >= timeout
        })
        .map(|(id, _)| id.clone())
        .collect();

    for agent_id in timed_out {
        warn!(agent_id, "health check timeout, deregistering agent");

        // Remove agent directory
        let agent_dir = self.agents_dir.join(&agent_id);
        let _ = fs::remove_dir_all(&agent_dir);

        // Remove from state
        self.agents.remove(&agent_id);
    }

    Ok(())
}
```

**Update event loop** (add after `check_periodic_health_checks()`):
```rust
state.check_health_check_timeouts()
    .map_err(|e| io::Error::new(e.kind(), format!("health check timeout failed: {e}")))?;
```

---

### Task 10: Update `complete_task()` to handle `HealthCheck`

**File:** `crates/agent_pool/src/daemon.rs` `complete_task()` (around line 505)

**Before:**
```rust
let InFlight::Task { respond_to } = in_flight;
let response = Response::processed(output);
send_response(respond_to, &response)?;
```

**After:**
```rust
match in_flight {
    InFlight::Task { respond_to } => {
        let response = Response::processed(output);
        send_response(respond_to, &response)?;
    }
    InFlight::HealthCheck => {
        // Health check completed successfully, nothing to send back
        debug!(agent_id, "health check response received");
    }
}
```

**Also:** Update `last_activity` when task completes. Add before the match:
```rust
agent.touch();
```

Wait, `agent` is borrowed. Need to restructure. Actually, we already set status to Idle via `std::mem::replace`. Let me check the current code structure.

**Actually:** The `agent` is mutably borrowed, so we can call `agent.touch()` after we set status to Idle. Add at the end of `complete_task()` before `Ok(())`:
```rust
if let Some(agent) = self.agents.get_mut(agent_id) {
    agent.touch();
}
```

---

### Task 11: Update shell scripts

**Files:**
- `crates/agent_pool/scripts/command-agent.sh`
- `crates/agent_pool/scripts/echo-agent.sh`
- `crates/agent_pool/scripts/greeting-agent.sh`

**Add at the start of the task processing loop:**
```bash
# Handle health checks
KIND=$(echo "$TASK_JSON" | jq -r '.kind // "Task"')
if [ "$KIND" = "HealthCheck" ]; then
    echo "[$NAME] Responding to health check" >&2
    echo "{}" > "$RESPONSE_FILE"
    continue
fi
```

---

### Task 12: Update demos

**Files:** `crates/agent_pool/demos/*.sh`

For simple demos, disable health checks:
```bash
agent_pool start --pool "$POOL" \
    --initial-health-check=false \
    --periodic-health-check=false &
```

---

### Task 13: Add tests

**File:** `crates/agent_pool/tests/health_check.rs`

Test cases:
- Initial health check sent on registration (when enabled)
- Agent status is `Busy(HealthCheck)` until response
- Periodic health check sent after interval (to idle agents only)
- Agent removed on health check timeout
- Agent can re-register after timeout
- Health checks disabled via config

---

## Summary

| Location | Change |
|----------|--------|
| `daemon.rs` InFlight enum | Add `HealthCheck` variant |
| `daemon.rs` DaemonConfig | Add 4 health check fields |
| `daemon.rs` AgentState | Add `last_activity: Instant` and `touch()` |
| `daemon.rs` register() | Dispatch initial health check |
| `daemon.rs` event_loop() | Call periodic health check and timeout check |
| `daemon.rs` complete_task() | Handle HealthCheck variant |
| `main.rs` Command::Start | Add 4 CLI flags |
| `main.rs` get_task | Pass through kind from task.json |
| Shell scripts | Handle HealthCheck kind |
| Demos | Disable health checks or handle them |
