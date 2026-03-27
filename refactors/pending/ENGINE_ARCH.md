A naive recursive AST evaluator (`Box::pin(evaluate(...))`) is a toy. It is sufficient for a local CLI tool, but it is structurally incapable of operating as a resilient, production-grade workflow engine. 

If you attempt to execute the AST by traversing the tree with asynchronous recursive function calls, you bind the logical state of your workflow directly to the Rust call stack (and Tokio's task memory). If the Rust process panics, is OOM-killed, or restarts, all inflight workflow state is permanently destroyed. You cannot pause workflows, you cannot resume them, and you cannot inspect their intermediate state without injecting heavy, intrusive tracing.

To build an engine that behaves like Cadence, Temporal, or AWS Step Functions, you must decouple the execution state from the call stack.

Here are the critical architectural decisions and failure modes you must navigate for the Rust execution engine.

### 1. The Execution Model: Event-Driven State Machine

The engine must not "execute" the AST directly. It must compile the AST into a flat execution graph, and a centralized event loop must step through that graph.

You represent the workflow not as a stack of futures, but as a persistent state machine. The engine is an actor (managed by Tokio) that reacts to discrete events.

* **The State:** A struct containing the workflow ID, the current node pointer, the accumulated context, and a history of completed node IDs.
* **The Loop:** A `tokio::sync::mpsc` channel receives events (`NodeCompleted`, `NodeFailed`, `NodeSpawned`). 
* **The Transition:** When a node finishes (e.g., Node returns data across the FFI), it fires a `NodeCompleted` event containing the JSON payload. The engine wakes up, updates the context, finds the topologically next node in the AST, and schedules it.

This architecture buys you durability. By writing every `NodeCompleted` event to a write-ahead log (WAL) or a database before advancing the state machine, you achieve Event Sourcing. If the engine crashes, you replay the event log to reconstruct the exact state machine and resume execution.

### 2. The FFI Boundary: Embedded Isolate vs. Daemon IPC

Crossing the Rust/JavaScript boundary is your primary latency bottleneck. How you architect this dictates your throughput.

**Do not spawn a new Node process per `Call` node.** The cold start time of the V8 engine and module resolution will dominate your execution profile, rendering the engine useless for high-throughput data processing.

You have two viable paths:

1.  **Embedded Isolate (`deno_core`):** You embed V8 directly into the Rust binary. Rust pushes the AST and data directly into the V8 heap using RustyV8 primitives. This is the highest performance architecture (zero serialization overhead for strings/numbers), but it binds the stability of your Rust process to the stability of the JavaScript user-land code. A segfault or memory leak in the JS handler takes down the entire engine.
2.  **Persistent Daemon IPC (Unix Domain Sockets):** You spin up a long-lived Node.js daemon alongside the Rust engine. They communicate over a Unix Domain Socket (or named pipes on Windows). Rust writes the target module, function, and JSON payload to the socket. Node executes and writes the JSON response back. This provides memory isolation and allows you to restart the Node worker independently if it leaks memory, at the cost of JSON serialization/deserialization overhead. 

For a resilient workflow engine, the Persistent Daemon IPC model via sockets is the correct starting point. It prevents user-land code from corrupting the host VM.

### 3. Concurrency and Cancellation: Tokio JoinSets

When executing a `Traverse` or `All` node, you will spawn multiple concurrent Tokio tasks. Managing the lifecycle of these tasks is notoriously easy to get wrong.

If you use `futures::future::join_all`, and one branch panics or errors, the other branches continue executing. You bleed resources computing results that will eventually be discarded.

You must use `tokio::task::JoinSet`. 

// rust-1
```rust
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

// Inside your Traverse or All execution block
let mut set = JoinSet::new();
let token = CancellationToken::new();

for item in items {
    let child_token = token.clone();
    set.spawn(async move {
        tokio::select! {
            _ = child_token.cancelled() => {
                // Graceful halt if a sibling fails
                Err(WorkflowError::Cancelled)
            }
            result = execute_node(item) => {
                result
            }
        }
    });
}

while let Some(res) = set.join_next().await {
    match res {
        Ok(Ok(val)) => { /* Store result */ },
        Ok(Err(e)) => {
            // A child failed. Cancel all other inflight tasks immediately.
            token.cancel(); 
            return Err(e);
        },
        Err(join_error) => { /* Handle task panic */ }
    }
}
```

A `JoinSet` ensures that if the parent task is dropped, all child tasks are aborted. The `CancellationToken` allows you to preemptively kill sibling tasks if one fails, preventing CPU starvation.

### 4. Memory Geometry: Zero-Copy Routing

The workflow algebra is highly structural. Primitives like `Sequence`, `Match`, and `Loop` do not mutate data; they merely route it. 

If you implement the engine using `serde_json::Value`, and you pass that value from `Node A` to `Sequence` to `Match` to `Loop` to `Node B`, standard Rust semantics will force you to `.clone()` the payload at multiple steps to satisfy the borrow checker, especially when spawning Tokio tasks. Cloning a 10MB JSON array to figure out which branch of a `Match` statement to take is architectural self-sabotage.

You must wrap the pipeline state in an `Arc` (Atomic Reference Count). 

// rust-2
```rust
use std::sync::Arc;
use serde_json::Value;

// The standard payload passed between execution boundaries
type State = Arc<Value>;
```

Routing nodes (`Match`, `Sequence`) only ever clone the `Arc` pointer, which is an $O(1)$ atomic increment. The underlying JSON payload is only deep-cloned when a native `Builtin` node (like `Merge` or `Tag`) must physically mutate the data structure. If a `Call` node needs to serialize the data for IPC, it reads the memory behind the `Arc` immutably.

### 5. Failure Modes to Anticipate

* **Head-of-Line Blocking in IPC:** If you route 1,000 parallel `Traverse` calls to a single Node.js daemon over a single socket, a slow handler will block the socket for fast handlers. Implement a multiplexed protocol (like JSON-RPC with ID tags) so Node can process and return results asynchronously over the same connection.
* **The Infinite Loop:** Your `Loop` primitive is mathematically sound, but user-land code is not. If a user writes a loop that never emits a `Break` signal, your engine will cycle forever. You must implement a deterministic instruction limit (e.g., maximum 10,000 transitions per workflow instance) or a hard wall-clock timeout enforced by `tokio::time::timeout` on the engine loop.
* **Deserialization Panics:** Never `.unwrap()` or `.expect()` on the structure of the JSON returned from the FFI boundary. User-land code can and will return `{ "foo": "bar" }` when the AST expects an array. Your engine must treat all incoming FFI data as hostile and map structural mismatches to a formal `WorkflowError::TypeMismatch` that gracefully aborts the pipeline, rather than a Rust panic that kills the Tokio runtime.