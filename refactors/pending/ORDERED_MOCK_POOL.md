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
/// Controller for releasing tasks in a specific order.
pub struct OrderedAgentController {
    /// Channel to send responses to waiting tasks.
    /// Each send() releases one waiting task.
    tx: mpsc::Sender<String>,
}

impl OrderedAgentController {
    /// Complete the next waiting task with this response JSON.
    pub fn complete(&self, response: &str) {
        self.tx.send(response.to_string()).expect("agent dropped");
    }

    /// Complete with empty array (terminate task, no children).
    pub fn terminate(&self) {
        self.complete("[]");
    }

    /// Complete with a single child task.
    pub fn spawn_one(&self, kind: &str) {
        self.complete(&format!(r#"[{{"kind": "{kind}", "value": {{}}}}]"#));
    }

    /// Complete with multiple child tasks.
    pub fn spawn(&self, kinds: &[&str]) {
        let tasks: Vec<String> = kinds
            .iter()
            .map(|k| format!(r#"{{"kind": "{k}", "value": {{}}}}"#))
            .collect();
        self.complete(&format!("[{}]", tasks.join(", ")));
    }
}
```

### New GsdTestAgent Method

```rust
impl GsdTestAgent {
    /// Start an agent that waits for explicit completion signals.
    ///
    /// Tasks block until `controller.complete()` is called.
    /// Tasks are released in FIFO order (first to arrive, first to complete).
    ///
    /// Returns (agent, controller).
    pub fn ordered(root: &Path) -> (Self, OrderedAgentController) {
        let (tx, rx) = mpsc::channel::<String>();
        let controller = OrderedAgentController { tx };

        let agent = Self::start(root, Duration::ZERO, move |_payload| {
            // Block until test sends response
            rx.recv().unwrap_or_else(|_| "[]".to_string())
        });

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

    // Distribute task arrives, complete it with 3 workers
    ctrl.spawn(&["Worker", "Worker", "Worker"]);

    // Workers arrive (order may vary), complete in specific order
    ctrl.terminate();  // Worker 1
    ctrl.terminate();  // Worker 2
    ctrl.terminate();  // Worker 3

    let output = handle.join().unwrap();
    // State log now has deterministic ordering
}
```

## Implementation Phases

### Phase 1: Add OrderedAgentController

**File:** `crates/gsd_cli/tests/common/mod.rs`

1. Add `OrderedAgentController` struct with `complete()`, `terminate()`, `spawn_one()`, `spawn()` methods
2. Add `GsdTestAgent::ordered(root) -> (Self, OrderedAgentController)`
3. Add basic test verifying ordered completion works

**Tests:**
```rust
#[test] fn ordered_agent_single_task()
#[test] fn ordered_agent_multiple_tasks_fifo()
#[test] fn ordered_agent_fan_out()
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

### Phase 3: Update Demos for Deterministic Testing

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
    // Assert state log matches snapshot
}

/// Run fan-out demo with reverse worker completion.
///
/// Tests that finally still works correctly regardless of child order.
#[test]
fn demo_fan_out_reverse_order() {
    // Same config, but complete workers in reverse order
}

/// Run branching demo with deterministic path.
#[test]
fn demo_branching_approve_path() {
    // Control which branch is taken
}

#[test]
fn demo_branching_reject_path() {
    // Control other branch
}
```

### Phase 4: Snapshot Testing Infrastructure

Add snapshot comparison utilities:

```rust
/// Compare state log against expected snapshot.
fn assert_log_matches_snapshot(log_path: &Path, snapshot_name: &str) {
    let actual = fs::read_to_string(log_path).unwrap();
    let expected_path = Path::new("tests/snapshots").join(format!("{snapshot_name}.ndjson"));

    if env::var("UPDATE_SNAPSHOTS").is_ok() {
        fs::write(&expected_path, &actual).unwrap();
        return;
    }

    let expected = fs::read_to_string(&expected_path)
        .unwrap_or_else(|_| panic!("snapshot not found: {}", expected_path.display()));

    assert_eq!(actual, expected, "state log differs from snapshot");
}
```

**Snapshots to create:**
- `tests/snapshots/fan_out_forward.ndjson`
- `tests/snapshots/fan_out_reverse.ndjson`
- `tests/snapshots/branching_approve.ndjson`
- `tests/snapshots/branching_reject.ndjson`
- `tests/snapshots/linear.ndjson`
- `tests/snapshots/hooks.ndjson`

## Testing the Refactor

After each phase:
1. `cargo test -p gsd_cli` - all existing tests still pass
2. New deterministic tests pass consistently (run 10x to verify no flakiness)

## Future Work

- Timeout handling for ordered agents (detect test bugs that forget to complete)
- Multi-agent ordered pools (multiple agents, controlled interleaving)
- Record/replay mode (record actual completion order, replay for debugging)
