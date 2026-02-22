# AgentStatus Enum Refactor

Replace `Option<InFlightTask>` with explicit `AgentStatus` enum for clear state modeling.

## Current State

**File:** `crates/agent_pool/src/daemon.rs`

```rust
// Lines 256-265
struct AgentState {
    in_flight: Option<InFlightTask>,
}

struct InFlightTask {
    respond_to: ResponseTarget,
}

// Lines 267-275
impl AgentState {
    const fn new() -> Self {
        Self { in_flight: None }
    }

    const fn is_available(&self) -> bool {
        self.in_flight.is_none()
    }
}
```

**Problems:**
- State is implicit (`None` = idle, `Some` = busy)
- Adding health checks would require a separate field or complex Option nesting
- Not self-documenting

## Target State

```rust
// New enums
enum AgentStatus {
    Idle,
    Busy(InFlight),
}

enum InFlight {
    Task { respond_to: ResponseTarget },
    // HealthCheck variant will be added later
}

// Updated struct
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

## Tasks

| Status | Task | Description |
|--------|------|-------------|
| [ ] | 1 | Add `AgentStatus` and `InFlight` enums |
| [ ] | 2 | Update `AgentState` struct |
| [ ] | 3 | Update `AgentState::new()` |
| [ ] | 4 | Rename `is_available()` to `is_idle()` |
| [ ] | 5 | Update `in_flight_count()` |
| [ ] | 6 | Update `scan_outputs()` |
| [ ] | 7 | Update `find_available_agent()` |
| [ ] | 8 | Update `dispatch_to()` |
| [ ] | 9 | Update `complete_task()` |
| [ ] | 10 | Run tests and verify |

---

## Task 1: Add `AgentStatus` and `InFlight` enums

**Location:** `daemon.rs` lines 256-265 (before `AgentState`)

**Add:**
```rust
/// The current status of an agent.
enum AgentStatus {
    /// Agent is idle, available for work.
    Idle,
    /// Agent is busy with an in-flight task.
    Busy(InFlight),
}

/// What the agent is currently working on.
enum InFlight {
    /// A real task from a submitter.
    Task { respond_to: ResponseTarget },
}
```

**Delete:** The old `InFlightTask` struct (lines 262-265):
```rust
// DELETE THIS:
struct InFlightTask {
    respond_to: ResponseTarget,
}
```

---

## Task 2: Update `AgentState` struct

**Location:** `daemon.rs` lines 256-260

**Before:**
```rust
struct AgentState {
    /// If busy, holds the stream to respond to when task completes.
    in_flight: Option<InFlightTask>,
}
```

**After:**
```rust
struct AgentState {
    status: AgentStatus,
}
```

---

## Task 3: Update `AgentState::new()`

**Location:** `daemon.rs` lines 268-270

**Before:**
```rust
const fn new() -> Self {
    Self { in_flight: None }
}
```

**After:**
```rust
const fn new() -> Self {
    Self { status: AgentStatus::Idle }
}
```

---

## Task 4: Rename `is_available()` to `is_idle()`

**Location:** `daemon.rs` lines 272-274

**Before:**
```rust
const fn is_available(&self) -> bool {
    self.in_flight.is_none()
}
```

**After:**
```rust
const fn is_idle(&self) -> bool {
    matches!(self.status, AgentStatus::Idle)
}
```

---

## Task 5: Update `in_flight_count()`

**Location:** `daemon.rs` lines 378-383

**Before:**
```rust
fn in_flight_count(&self) -> usize {
    self.agents
        .values()
        .filter(|a| a.in_flight.is_some())
        .count()
}
```

**After:**
```rust
fn in_flight_count(&self) -> usize {
    self.agents
        .values()
        .filter(|a| matches!(a.status, AgentStatus::Busy(_)))
        .count()
}
```

---

## Task 6: Update `scan_outputs()`

**Location:** `daemon.rs` lines 385-412

**Before (lines 386-391):**
```rust
let busy: Vec<_> = self
    .agents
    .iter()
    .filter(|(_, a)| a.in_flight.is_some())
    .map(|(id, _)| id.clone())
    .collect();
```

**After:**
```rust
let busy: Vec<_> = self
    .agents
    .iter()
    .filter(|(_, a)| matches!(a.status, AgentStatus::Busy(_)))
    .map(|(id, _)| id.clone())
    .collect();
```

---

## Task 7: Update `find_available_agent()`

**Location:** `daemon.rs` lines 448-471

**Before (line 453):**
```rust
.find(|(id, a)| a.is_available() && self.agents_dir.join(id).is_dir())
```

**After:**
```rust
.find(|(id, a)| a.is_idle() && self.agents_dir.join(id).is_dir())
```

**Before (line 461):**
```rust
.filter(|(id, a)| a.is_available() && !self.agents_dir.join(id).is_dir())
```

**After:**
```rust
.filter(|(id, a)| a.is_idle() && !self.agents_dir.join(id).is_dir())
```

---

## Task 8: Update `dispatch_to()`

**Location:** `daemon.rs` lines 473-487

**Before (lines 483-485):**
```rust
agent.in_flight = Some(InFlightTask {
    respond_to: task.respond_to,
});
```

**After:**
```rust
agent.status = AgentStatus::Busy(InFlight::Task {
    respond_to: task.respond_to,
});
```

---

## Task 9: Update `complete_task()`

**Location:** `daemon.rs` lines 489-518

This is the most complex change. The current code uses `Option::take()` to atomically remove and return the in-flight task:

**Before (lines 494-496):**
```rust
let Some(in_flight) = agent.in_flight.take() else {
    return Ok(());
};
```

**After:**
```rust
let AgentStatus::Busy(in_flight) = std::mem::replace(&mut agent.status, AgentStatus::Idle) else {
    return Ok(());
};
```

**Before (lines 500-502) - restoring on error:**
```rust
Err(e) if e.kind() == io::ErrorKind::NotFound => {
    agent.in_flight = Some(in_flight);
    return Ok(());
}
```

**After:**
```rust
Err(e) if e.kind() == io::ErrorKind::NotFound => {
    agent.status = AgentStatus::Busy(in_flight);
    return Ok(());
}
```

**Before (line 514) - using respond_to:**
```rust
send_response(in_flight.respond_to, &response)?;
```

**After:**
```rust
let InFlight::Task { respond_to } = in_flight;
send_response(respond_to, &response)?;
```

Note: When we add `InFlight::HealthCheck`, this will need to handle both variants. For now, the `let` pattern is irrefutable since `Task` is the only variant.

---

## Task 10: Run tests and verify

```bash
cargo test -p agent_pool
cargo check -p agent_pool
```

All existing tests should pass unchanged since we're only refactoring internal representation.

---

## Summary of Changes

| Location | Lines | Change |
|----------|-------|--------|
| New enums | 256+ | Add `AgentStatus` and `InFlight` enums |
| `InFlightTask` | 262-265 | Delete (replaced by `InFlight::Task`) |
| `AgentState` | 256-260 | Replace `in_flight: Option<InFlightTask>` with `status: AgentStatus` |
| `AgentState::new()` | 268-270 | Return `AgentStatus::Idle` |
| `is_available()` | 272-274 | Rename to `is_idle()`, use `matches!` |
| `in_flight_count()` | 378-383 | Use `matches!(a.status, AgentStatus::Busy(_))` |
| `scan_outputs()` | 386-391 | Use `matches!(a.status, AgentStatus::Busy(_))` |
| `find_available_agent()` | 453, 461 | Call `is_idle()` instead of `is_available()` |
| `dispatch_to()` | 483-485 | Set `agent.status = AgentStatus::Busy(InFlight::Task {...})` |
| `complete_task()` | 494-514 | Use `std::mem::replace`, destructure `InFlight::Task` |
