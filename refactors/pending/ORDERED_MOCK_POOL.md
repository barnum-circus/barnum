# Ordered Mock Pool for Deterministic Tests

**Status:** Not started

## Motivation

Current tests use `GsdTestAgent` with `processing_delay` (time-based delays) to simulate agent response times. This works for basic tests but fails for deterministic snapshot testing:

1. **Fan-out tests are non-deterministic** - When multiple tasks run concurrently, completion order depends on timing
2. **Snapshot tests require stable output** - State logs must be identical across runs
3. **Complex scenarios need controlled ordering** - Testing retry + finally + fan-out interactions requires precise control

## Current Test Infrastructure

`GsdTestAgent` in `crates/gsd_cli/tests/common/mod.rs`:

```rust
impl GsdTestAgent {
    pub fn start<F>(root: &Path, processing_delay: Duration, processor: F) -> Self
    where
        F: Fn(&str) -> String + Send + 'static,
    {
        // ... spawns thread that:
        // 1. Waits for task assignment via inotify
        // 2. Sleeps for processing_delay
        // 3. Calls processor(payload) to get response
        // 4. Writes response
    }

    pub fn terminator(root: &Path, processing_delay: Duration) -> Self { ... }
    pub fn transition_to(root: &Path, processing_delay: Duration, next_kind: &str) -> Self { ... }
    pub fn with_transitions(root: &Path, processing_delay: Duration, transitions: Vec<(&str, &str)>) -> Self { ... }
}
```

The `processor` closure generates responses synchronously. Time-based ordering via `processing_delay` is inherently racy.

## Proposed Solution

Add `OrderedAgentController` that lets tests explicitly control when each task completes.

### New Types

```rust
use std::sync::{Arc, Mutex, mpsc};

/// A waiting task that hasn't been completed yet.
pub struct WaitingTask {
    /// The task kind (e.g., "Worker", "Analyze")
    pub kind: String,
    /// The full payload JSON
    pub payload: String,
    /// Channel to send the response
    response_tx: oneshot::Sender<String>,
}

/// Controller for completing tasks in any order.
///
/// Minimal API: wait for tasks, inspect them, complete by index.
pub struct OrderedAgentController {
    /// Tasks waiting for completion.
    waiting: Arc<Mutex<Vec<WaitingTask>>>,
    /// Channel that receives notifications when tasks arrive.
    arrival_rx: mpsc::Receiver<()>,
}

impl OrderedAgentController {
    /// Block until at least `count` tasks are waiting.
    pub fn wait_for_tasks(&self, count: usize) {
        loop {
            {
                let waiting = self.waiting.lock().unwrap();
                if waiting.len() >= count {
                    return;
                }
            }
            // Block until a task arrives
            self.arrival_rx.recv().expect("agent dropped");
        }
    }

    /// Get list of currently waiting tasks (kind, payload).
    pub fn waiting_tasks(&self) -> Vec<(String, String)> {
        let waiting = self.waiting.lock().unwrap();
        waiting.iter().map(|t| (t.kind.clone(), t.payload.clone())).collect()
    }

    /// Complete a specific waiting task by index.
    pub fn complete_at(&self, index: usize, response: &str) {
        let task = {
            let mut waiting = self.waiting.lock().unwrap();
            waiting.remove(index)
        };
        let _ = task.response_tx.send(response.to_string());
    }
}
```

### New GsdTestAgent Method

```rust
impl GsdTestAgent {
    /// Start an agent that waits for explicit completion signals.
    ///
    /// Tasks register with the controller when they arrive and block
    /// until the test explicitly completes them. Tests can complete
    /// tasks in any order by index.
    ///
    /// Returns (agent, controller).
    pub fn ordered(root: &Path) -> (Self, OrderedAgentController) {
        let waiting: Arc<Mutex<Vec<WaitingTask>>> = Arc::new(Mutex::new(Vec::new()));
        let (arrival_tx, arrival_rx) = mpsc::channel::<()>();

        let waiting_clone = waiting.clone();

        let agent = Self::start(root, Duration::ZERO, move |payload| {
            // Parse task kind from payload
            let kind = serde_json::from_str::<serde_json::Value>(payload)
                .ok()
                .and_then(|v| v.get("task")?.get("kind")?.as_str().map(String::from))
                .unwrap_or_else(|| "Unknown".to_string());

            // Create oneshot channel for this task's response
            let (tx, rx) = oneshot::channel();

            // Register as waiting
            {
                let mut waiting = waiting_clone.lock().unwrap();
                waiting.push(WaitingTask {
                    kind,
                    payload: payload.to_string(),
                    response_tx: tx,
                });
            }

            // Notify controller that a task arrived
            let _ = arrival_tx.send(());

            // Block until test sends response
            rx.recv().unwrap_or_else(|_| "[]".to_string())
        });

        let controller = OrderedAgentController {
            waiting,
            arrival_rx,
        };

        (agent, controller)
    }
}
```

### Usage Example

```rust
#[test]
fn fan_out_deterministic_order() {
    let root = setup_test_dir("fan_out_ordered");
    let pool = AgentPoolHandle::start(&root.join("pool"));
    let (agent, ctrl) = GsdTestAgent::ordered(&root.join("pool"));

    // Config: Distribute -> Worker (fan-out)
    let config = r#"{ "steps": [
        {"name": "Distribute", "action": {"kind": "Pool", ...}, "next": ["Worker"]},
        {"name": "Worker", "action": {"kind": "Pool", ...}, "next": []}
    ]}"#;

    let gsd = GsdRunner::new();

    // Start GSD in background
    let handle = thread::spawn(move || {
        gsd.run(config, r#"[{"kind": "Distribute", "value": {}}]"#, &root.join("pool"))
    });

    // Wait for Distribute task
    ctrl.wait_for_tasks(1);
    assert_eq!(ctrl.waiting_tasks()[0].0, "Distribute");

    // Complete Distribute, spawning 3 workers
    ctrl.complete_at(0, r#"[
        {"kind": "Worker", "value": {"id": 1}},
        {"kind": "Worker", "value": {"id": 2}},
        {"kind": "Worker", "value": {"id": 3}}
    ]"#);

    // Wait for all 3 workers to arrive
    ctrl.wait_for_tasks(3);

    let tasks = ctrl.waiting_tasks();
    assert_eq!(tasks.len(), 3);
    assert!(tasks.iter().all(|(kind, _)| kind == "Worker"));

    // Complete in reverse order (or any order we want)
    ctrl.complete_at(2, "[]");  // Worker 3 first
    ctrl.complete_at(1, "[]");  // Worker 2
    ctrl.complete_at(0, "[]");  // Worker 1 last

    let output = handle.join().unwrap();
    // State log has deterministic ordering based on completion order
}
```

## Implementation Phases

### Phase 1: Add OrderedAgentController

**File:** `crates/gsd_cli/tests/common/mod.rs`

1. Add `WaitingTask` struct and `OrderedAgentController` with:
   - `wait_for_tasks(count)` - block until count tasks waiting
   - `waiting_tasks()` - inspect waiting tasks
   - `complete_at(index, response)` - complete by index
2. Add `GsdTestAgent::ordered(root) -> (Self, OrderedAgentController)`
3. Add basic tests verifying ordered completion works

**Tests:**
```rust
#[test] fn ordered_agent_single_task()
#[test] fn ordered_agent_wait_for_multiple()
#[test] fn ordered_agent_complete_out_of_order()
#[test] fn ordered_agent_waiting_tasks_query()
```

### Phase 2: Add Payload-Aware Completion

The basic `ordered()` ignores the payload. For more control, add payload inspection:

```rust
impl GsdTestAgent {
    /// Start an ordered agent that exposes received payloads.
    ///
    /// Returns (agent, controller) where controller can inspect payloads.
    pub fn ordered_with_payloads(root: &Path) -> (Self, PayloadAwareController) {
        // Implementation uses two channels:
        // 1. payload_tx: agent -> test (sends payload when task arrives)
        // 2. response_rx: test -> agent (receives response to send)
    }
}

pub struct PayloadAwareController {
    payload_rx: mpsc::Receiver<String>,
    response_tx: mpsc::Sender<String>,
}

impl PayloadAwareController {
    /// Wait for next task and return its payload.
    pub fn next_payload(&self) -> String {
        self.payload_rx.recv().expect("agent dropped")
    }

    /// Complete the current task with this response.
    pub fn complete(&self, response: &str) {
        self.response_tx.send(response.to_string()).expect("agent dropped");
    }

    /// Wait for task, inspect payload, then complete.
    pub fn handle<F>(&self, f: F)
    where
        F: FnOnce(&str) -> String,
    {
        let payload = self.next_payload();
        let response = f(&payload);
        self.complete(&response);
    }
}
```

**Tests:**
```rust
#[test] fn payload_aware_inspects_task_kind()
#[test] fn payload_aware_conditional_response()
```

### Phase 3: Add Deterministic Demo Tests

Create test wrappers for existing demos that use ordered completion:

**File:** `crates/gsd_cli/tests/demo_deterministic.rs`

```rust
/// Run fan-out demo with controlled ordering.
///
/// Order: Distribute completes, then workers complete in ID order.
#[test]
fn demo_fan_out_deterministic() {
    // Use same config as demos/fan-out/config.jsonc
    // Use OrderedAgentController to control completion order
    // Verify all tasks complete successfully
}

/// Run fan-out demo with reverse worker completion.
///
/// Tests that finally still works correctly regardless of child order.
#[test]
fn demo_fan_out_reverse_order() {
    // Same config, but complete workers in reverse order
    // Verify finally hook runs after all workers regardless of order
}

/// Run branching demo with deterministic path.
#[test]
fn demo_branching_approve_path() {
    // Control which branch is taken by controlling agent response
}

#[test]
fn demo_branching_reject_path() {
    // Control other branch
}
```

## Testing the Refactor

After each phase:
1. `cargo test -p gsd_cli` - all existing tests still pass
2. New deterministic tests pass consistently (run 10x to verify no flakiness)

## Future Work

- Timeout handling for ordered agents (detect test bugs that forget to complete)
- Multi-agent ordered pools (multiple agents, controlled interleaving)
- Record/replay mode (record actual completion order, replay for debugging)
- **Snapshot testing** - Once STATE_PERSISTENCE is implemented, use deterministic ordering for state log snapshot tests
