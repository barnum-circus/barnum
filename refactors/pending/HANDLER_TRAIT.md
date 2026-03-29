# Handler trait + registration system

## Current state

The `HandlerKind` enum in `barnum_ast` is a closed discriminated union:

```rust
pub enum HandlerKind {
    TypeScript(TypeScriptHandler),
    Builtin(BuiltinHandler),
}
```

The scheduler (`barnum_event_loop`) matches on this enum and dispatches to the appropriate execution function. Adding a new handler type (e.g., Python, Wasm, shell script) requires modifying:

1. `HandlerKind` in `barnum_ast` — add a variant
2. `Scheduler::dispatch` in `barnum_event_loop` — add a match arm
3. A new crate for the handler execution logic

The enum is serialized as a tagged union in JSON (`#[serde(tag = "kind")]`), so the TypeScript config builder also needs to know about each handler type.

## Proposed design

### Handler trait

```rust
#[async_trait]
pub trait Handler: Send + Sync {
    /// Execute the handler with the given input value.
    async fn execute(&self, value: &Value) -> Result<Value, HandlerError>;
}
```

Each handler type implements this trait:

```rust
// barnum_builtins
impl Handler for BuiltinHandler {
    async fn execute(&self, value: &Value) -> Result<Value, HandlerError> {
        execute_builtin(&self.builtin, value).map_err(HandlerError::from)
    }
}

// barnum_typescript_handler
impl Handler for TypeScriptHandler {
    async fn execute(&self, value: &Value) -> Result<Value, HandlerError> {
        execute_typescript(&self.executor, &self.worker_path, &self.module, &self.func, value)
            .await
            .map_err(HandlerError::from)
    }
}
```

### Registration

The scheduler stops matching on `HandlerKind` and instead receives a `Box<dyn Handler>`:

```rust
pub struct Scheduler {
    result_tx: mpsc::UnboundedSender<(TaskId, Result<Value, HandlerError>)>,
    result_rx: mpsc::UnboundedReceiver<(TaskId, Result<Value, HandlerError>)>,
}

impl Scheduler {
    pub fn dispatch(&self, task_id: TaskId, value: Value, handler: &dyn Handler) {
        // ... spawn tokio task calling handler.execute(value)
    }
}
```

### Handler registry

A registry maps `HandlerKind` (still a serialized enum for the config format) to `Box<dyn Handler>`:

```rust
pub struct HandlerRegistry {
    typescript: TypeScriptHandlerFactory,
    // Future: python, wasm, etc.
}

impl HandlerRegistry {
    pub fn resolve(&self, kind: &HandlerKind) -> Box<dyn Handler> {
        match kind {
            HandlerKind::TypeScript(ts) => self.typescript.create(ts),
            HandlerKind::Builtin(b) => Box::new(b.clone()),
        }
    }
}
```

The `TypeScriptHandlerFactory` holds the executor path and worker path (currently on `Scheduler`), and creates handler instances per invocation.

## Open questions

### Where does `HandlerKind` live?

Currently in `barnum_ast` because it's part of the serialized config. If we add the `Handler` trait, does `barnum_ast` depend on the trait? No — `barnum_ast` defines the data model (what gets serialized), and the trait lives in a separate crate (`barnum_handler`?). The registry translates from data model to trait objects.

### Should builtins use the trait?

Builtins are pure functions — no async, no state. Wrapping them in an `async fn` that immediately returns is fine but unnecessary overhead. Alternative: the scheduler special-cases builtins (they're always inline) and only uses the trait for external handlers.

Counter-argument: uniformity. Having one code path simplifies the scheduler and makes builtins testable via the same interface.

### Error type unification

Currently each handler type has its own error (`BuiltinError`, `TypeScriptHandlerError`). The `HandlerError` enum in `barnum_event_loop` wraps both. With a trait, either:

- The trait returns `Box<dyn Error>` — erases type info but maximally flexible
- The trait returns a `HandlerError` enum — every handler must convert to it
- The trait has an associated error type — most precise but prevents trait objects

Recommendation: `HandlerError` as a `#[non_exhaustive]` enum. Each handler crate provides a `From` impl. This preserves error specificity while keeping trait objects usable.

## Implementation order

1. Create `barnum_handler` crate with the `Handler` trait
2. Implement `Handler` for `BuiltinHandler` in `barnum_builtins`
3. Implement `Handler` for `TypeScriptHandler` in `barnum_typescript_handler`
4. Add `HandlerRegistry` to `barnum_event_loop`
5. Refactor `Scheduler` to accept `&dyn Handler` instead of `&HandlerKind`
6. Remove the direct dependency from `barnum_event_loop` → `barnum_builtins` and `barnum_typescript_handler` (they're accessed via the registry)

## What this enables

- Adding new handler types without modifying the scheduler
- Testing workflows with mock handlers (`struct MockHandler { output: Value }`)
- Plugin-style handler registration for third-party handler types
