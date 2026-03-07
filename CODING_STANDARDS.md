# Coding Standards

## Core Principles

### 1. Simple Primitives Above All Else

Complexity is the enemy. Before adding a feature, ask:
- Can this be expressed with existing primitives?
- Is there a simpler abstraction that covers this and other use cases?
- Would a user be able to explain this in one sentence?

If the implementation feels complicated, the abstraction is probably wrong. Step back and find the simpler primitive.

### 2. Make Impossible States Unrepresentable

Use the type system to enforce invariants at compile time.

```rust
// BAD: Impossible state representable
struct Task {
    state: TaskState,
    pending_count: u32,  // Only valid when state == AwaitingDescendants
}

// GOOD: Impossible state unrepresentable
enum TaskState {
    Pending,
    AwaitingDescendants(NonZeroU16),  // Count is part of the variant
}
```

```rust
// BAD: Can construct invalid state
struct Config {
    max_retries: u32,
    current_retries: u32,  // Could be > max_retries
}

// GOOD: Invariant enforced by type
struct Retries {
    current: u32,
    max: u32,
}

impl Retries {
    fn new(max: u32) -> Self { Self { current: 0, max } }
    fn increment(&mut self) -> bool {
        if self.current < self.max {
            self.current += 1;
            true
        } else {
            false
        }
    }
}
```

### 3. Newtypes Everywhere

Wrap primitive types to prevent mixing them up.

```rust
// BAD: Easy to confuse task_id with parent_id
fn process(task_id: u64, parent_id: u64) { ... }

// GOOD: Type system prevents confusion
#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
struct TaskId(pub u64);

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
struct ParentId(pub u64);

fn process(task_id: TaskId, parent_id: ParentId) { ... }
```

Newtypes are free at runtime (zero-cost abstraction) but catch bugs at compile time.

### 4. No Timeouts

Never use timeouts to work around race conditions or synchronization issues.

```rust
// BAD: Timeout hiding a race condition
loop {
    if check_condition() { break; }
    thread::sleep(Duration::from_millis(100));
    if elapsed > timeout { panic!("timed out"); }
}

// GOOD: Proper synchronization
let result = receiver.recv();  // Blocks until ready
```

Timeouts:
- Hide bugs instead of fixing them
- Make tests flaky
- Are arbitrary (why 100ms? why not 50ms or 200ms?)

If you think you need a timeout, you probably need a channel, condvar, or better architecture.

### 5. Small Testable Core, Mutable State on the Outside

Structure code as:
- **Pure core**: Business logic, deterministic, easy to test
- **Impure shell**: I/O, state, side effects

```rust
// BAD: Mixed concerns
impl Runner {
    fn process(&mut self) {
        let data = self.fetch_from_network();  // I/O
        let result = complex_business_logic(data);  // Pure
        self.state.update(result);  // Mutation
        self.send_to_network(result);  // I/O
    }
}

// GOOD: Separated concerns
// Pure core - easy to test
fn process_task(task: &Task, config: &Config) -> TaskResult {
    // All business logic here, no I/O, no mutation
}

// Impure shell - thin wrapper
impl Runner {
    fn run(&mut self) {
        let task = self.fetch_task();
        let result = process_task(&task, &self.config);  // Pure!
        self.apply_result(result);
    }
}
```

The pure core can be tested without mocks, network, or timing issues.

---

## Specific Patterns

### Error Handling

Use `thiserror` for error enums:

```rust
#[derive(Debug, thiserror::Error)]
enum ProcessError {
    #[error("task {0:?} not found")]
    TaskNotFound(TaskId),
    #[error("invalid state transition from {from:?} to {to:?}")]
    InvalidTransition { from: State, to: State },
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

### Serde Tagging

Use internally tagged enums with `kind`:

```rust
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind")]
enum Event {
    TaskSubmitted(TaskSubmitted),
    TaskCompleted(TaskCompleted),
}
```

Produces: `{"kind": "TaskSubmitted", "task_id": 1, ...}`

### NonZero Types

Use `NonZeroU*` when zero is invalid:

```rust
// Count that's only stored when > 0
enum State {
    Done,
    Waiting(NonZeroU16),  // Zero not representable
}
```

### Builder Pattern

For complex construction with validation:

```rust
impl Config {
    pub fn builder() -> ConfigBuilder {
        ConfigBuilder::default()
    }
}

impl ConfigBuilder {
    pub fn max_retries(mut self, n: u32) -> Self {
        self.max_retries = n;
        self
    }

    pub fn build(self) -> Result<Config, ConfigError> {
        // Validate and construct
    }
}
```

---

## Testing

### Test the Pure Core

Focus tests on the pure business logic:

```rust
#[test]
fn process_task_spawns_children() {
    let task = Task::new("Process", json!({"x": 1}));
    let config = Config::default();

    let result = process_task(&task, &config);

    assert_eq!(result.spawned.len(), 2);
}
```

### No Sleep in Tests

Tests must not use `thread::sleep` or timeouts:

```rust
// BAD
#[test]
fn test_async_thing() {
    start_async_operation();
    thread::sleep(Duration::from_secs(1));  // Flaky!
    assert!(is_done());
}

// GOOD
#[test]
fn test_async_thing() {
    let (tx, rx) = channel();
    start_async_operation(tx);
    let result = rx.recv().unwrap();  // Blocks until done
    assert!(result.is_ok());
}
```

### Property-Based Testing

For complex logic, consider proptest:

```rust
proptest! {
    #[test]
    fn retries_never_exceed_max(max in 0u32..100) {
        let mut retries = Retries::new(max);
        for _ in 0..1000 {
            retries.increment();
        }
        assert!(retries.current <= retries.max);
    }
}
```
