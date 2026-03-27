# Workflow Algebra

## Mental Model

TypeScript is the compiler. Rust is the VM.

TypeScript combinators are AST constructors (builder pattern) that produce a JSON data structure—a program in a small DSL. JavaScript closures cannot cross the serialization boundary to Rust, so the combinators build data, not functions. The output of a workflow declaration is a JSON object.

Rust reads this AST and interprets it: dispatching to handlers, threading data between nodes, managing concurrency, and enforcing the loop protocol.

Leaf nodes reference exported functions by module path and name—the same pattern used by Temporal and Cadence for distributed execution. `fromConfig` resolves imported Handler objects into these references. See `refactors/past/OPAQUE_HANDLER.md`.

## Primitives

Eight AST node types. One leaf computation (Call). Three compositional (Sequence, Traverse, All). One routing (Match). One iteration (Loop). One error materialization (Attempt). One named reference (Step).

Each primitive is specified in four dimensions: concept, TypeScript builder API, serialized JSON form, and Rust evaluation semantics.

The Rust evaluator has this shape:

```rust
async fn evaluate(
    action: &Action,
    input: Value,
    context: &Value,
    steps: &HashMap<String, Action>,
) -> Result<Value> {
    match action {
        // ... one arm per primitive
    }
}
```

The `context` parameter is a read-only environment passed by reference through the entire execution tree. It carries global state (API keys, workflow IDs, tenant config) without flowing through the data pipeline. Handlers receive `(input, context)` at the FFI boundary. This avoids the O(N) cloning cost of threading context through the DAG topology via All+Identity+Merge.

### Call

Leaf node. The only primitive that executes external code. Contains a `handler` field discriminated by `HandlerKind`, currently only `TypeScript`. Future handler types (Bash, Python) add variants to `HandlerKind`, not new AST nodes.

Categorically: a morphism in the Kleisli category (A -> M B).

**TS builder:**

```ts
call("./handlers.ts", "analyze")

// Or via Handler import (resolved by fromConfig):
import analyzeHandler from "./handlers/analyze.js";
sequence(analyzeHandler, ...)
```

**Serialized:**

```json
{
  "kind": "Call",
  "handler": {
    "kind": "TypeScript",
    "module": "/abs/path/handlers.ts",
    "func": "analyze",
    "stepConfig": null,
    "valueSchema": null
  }
}
```

**Rust evaluation:**

```rust
Action::Call { module, func, .. } => {
    node_runner::execute(module, func, input, context).await
}
```

### Sequence

Sequential composition. Each action receives the previous action's output.

Categorically: Kleisli composition (>=>).

**TS builder:**

```ts
sequence(a, b, c)
```

**Serialized:**

```json
{ "kind": "Sequence", "actions": [a, b, c] }
```

**Rust evaluation:**

```rust
Action::Sequence { actions } => {
    let mut state = input;
    for action in actions {
        state = evaluate(action, state, context, steps).await?;
    }
    Ok(state)
}
```

Failure at any point aborts the sequence.

### Traverse

Parallel map over an array. Applies an action to each element concurrently, preserving order. Input must be a JSON array.

Categorically: traverse over List with the Promise applicative. `List(A)` → (via `A → M(B)`) → `M(List(B))`.

**TS builder:**

```ts
traverse(action)
```

**Serialized:**

```json
{ "kind": "Traverse", "action": action }
```

**Rust evaluation:**

```rust
Action::Traverse { action } => {
    let items = input.as_array()
        .expect("Traverse input must be an array");
    let results = stream::iter(items)
        .map(|item| evaluate(action, item.clone(), context, steps))
        .buffer_unordered(concurrency_limit)
        .try_collect::<Vec<_>>()
        .await?;
    Ok(Value::Array(results))
}
```

`try_join_all` would spawn unbounded concurrent tasks — 50,000 files means 50,000 simultaneous FFI calls, exhausting file descriptors and IPC capacity. `buffer_unordered` enforces a concurrency ceiling. The limit is configurable (step-level or global).

### All

Passes the same input to multiple independent actions in parallel. Collects results as a JSON array.

Categorically: applicative zip / arrow fanout.

**TS builder:**

```ts
all(a, b, c)
```

**Serialized:**

```json
{ "kind": "All", "actions": [a, b, c] }
```

**Rust evaluation:**

```rust
Action::All { actions } => {
    let futures: Vec<_> = actions.iter()
        .map(|action| evaluate(action, input.clone(), context, steps))
        .collect();
    let results = futures::future::try_join_all(futures).await?;
    Ok(Value::Array(results))
}
```

Each action receives the same input. Output is a JSON array of results in the same order as the actions.

### Match

N-ary coproduct eliminator. Routes execution based on the `kind` field of a discriminated union. The `cases` map provides an action for each variant.

TypeScript enforces exhaustive handling via distributive conditional types over the union's `kind` literals:

```ts
type MatchCases<U extends { kind: string }, Out> = {
  [K in U['kind']]: Action  // must cover every variant
};
```

**TS builder:**

```ts
match({
  HasErrors: handleErrorsAction,
  Clean: handleCleanAction,
})
```

**Serialized:**

```json
{
  "kind": "Match",
  "cases": {
    "HasErrors": { "kind": "Call", "module": "...", "func": "..." },
    "Clean": { "kind": "Call", "module": "...", "func": "..." }
  }
}
```

**Rust evaluation:**

```rust
Action::Match { cases } => {
    let variant_kind = input.get("kind")
        .and_then(|v| v.as_str())
        .expect("Match input must have a 'kind' field");
    let action = cases.get(variant_kind)
        .unwrap_or_else(|| panic!("No match case for kind '{}'", variant_kind));
    evaluate(action, input, context, steps).await
}
```

The action receives the full variant object (including its `kind` field). If the `kind` value has no matching case, evaluation fails.

### Loop

Monadic fixed-point. Repeatedly executes a body action until it signals completion. The body must produce output with `kind: "Continue"` (iterate with new state) or `kind: "Break"` (exit with result). Both carry a `value` field. This mirrors Rust's `ControlFlow` enum.

Categorically: tailRecM from MonadRec. Stack-safe monadic iteration.

**TS builder:**

```ts
loop(body)
```

**Serialized:**

```json
{ "kind": "Loop", "body": bodyAction }
```

**Rust evaluation:**

```rust
Action::Loop { body } => {
    let mut state = input;
    loop {
        let result = evaluate(body, state, context, steps).await?;
        let signal = result.get("kind")
            .and_then(|v| v.as_str())
            .expect("Loop body must produce {kind: \"Continue\"|\"Break\", value}");
        let value = result.get("value")
            .expect("Loop body must produce {kind, value}")
            .clone();
        match signal {
            "Break" => return Ok(value),
            "Continue" => state = value,
            other => panic!("Loop body produced unknown kind '{}'", other),
        }
    }
}
```

The loop's initial state is its pipeline input. Continue's value becomes the next iteration's input. Break's value is the loop's output.

#### Loop signals (value domain, not AST nodes)

Continue and Break are values, not AST primitives. The loop body must produce output with `kind: "Continue"` or `kind: "Break"`. How that value gets constructed is the handler's concern.

The TypeScript builder provides convenience functions `recur()` and `done()` that produce Call nodes to built-in signal handlers:

```ts
recur()  // Call to a handler that wraps input as { kind: "Continue", value: input }
done()   // Call to a handler that wraps input as { kind: "Break", value: input }
```

These are syntactic sugar. A handler can produce `{kind: "Continue", value}` or `{kind: "Break", value}` directly without using `recur()`/`done()`.

### Attempt

Error materialization. Executes an action and reifies both success and failure into a discriminated union, making the node infallible from the VM's perspective. The output is always `{kind: "Success", value}` or `{kind: "Failure", error, input}`.

This separates error handling (catching) from error routing (branching). Attempt catches; Match routes. The previous `Recover` design conflated both by embedding a fallback action, duplicating the branching responsibility that belongs to Match.

Categorically: `attempt` / `materialize` / `either` from effect systems.

**TS builder:**

```ts
attempt(action)
```

**Serialized:**

```json
{ "kind": "Attempt", "action": innerAction }
```

**Rust evaluation:**

```rust
Action::Attempt { action } => {
    match evaluate(action, input.clone(), context, steps).await {
        Ok(result) => Ok(json!({
            "kind": "Success",
            "value": result,
        })),
        Err(error) => Ok(json!({
            "kind": "Failure",
            "error": error.to_string(),
            "input": input,
        })),
    }
}
```

Composed with Match for error routing:

```ts
sequence(
  attempt(fetchOrders),
  match({
    Success: processOrders,
    Failure: defaultOrders,
  }),
)
```

The developer must explicitly handle both branches via Match's exhaustive cases. No errors silently vanish into a catch-all.

### Step

Invokes a named step. Dispatches the current value to the step's action, evaluates it, and returns the result. Named steps are Kleisli arrows with names, callable from multiple points in the AST.

Needed for mutual recursion and DAG topologies where tree-shaped composition breaks down.

**TS builder:**

```ts
step("TypeCheck")
```

**Serialized:**

```json
{ "kind": "Step", "step": "TypeCheck" }
```

**Rust evaluation:**

```rust
Action::Step { step: step_name } => {
    let action = steps.get(step_name)
        .unwrap_or_else(|| panic!("Undefined step '{}'", step_name));
    evaluate(action, input, context, steps).await
}
```

### Builtin

Rust-native data transformation. Executes entirely in the VM without FFI. Used for structural operations that shape data between handler calls.

Operations:
- **Tag(kind):** Wraps input as `{kind, value: input}`. Used for loop signals (`recur() = Tag("Continue")`, `done() = Tag("Break")`) and any discriminated union construction.
- **Identity:** Passes input through unchanged. Used for the Arrow problem (preserving context across destructive nodes).
- **Merge:** Merges an array of objects into a single object. `[{a:1}, {b:2}]` becomes `{a:1, b:2}`.
- **Flatten:** Flattens a nested array one level. `[[1,2], [3]]` becomes `[1,2,3]`.
- **ExtractField(field):** Extracts a field from an object. `{a:1, b:2}` with field "a" becomes `1`.

**TS builders:**

```ts
identity()                           // Builtin Identity
merge()                              // Builtin Merge
flatten()                            // Builtin Flatten
extractField("errors")               // Builtin ExtractField
tag("Continue")                      // Builtin Tag
recur()                              // tag("Continue")
done()                               // tag("Break")
```

**Serialized:**

```json
{ "kind": "Builtin", "op": { "type": "Tag", "kind": "Continue" } }
{ "kind": "Builtin", "op": { "type": "Identity" } }
{ "kind": "Builtin", "op": { "type": "Merge" } }
{ "kind": "Builtin", "op": { "type": "Flatten" } }
{ "kind": "Builtin", "op": { "type": "ExtractField", "field": "errors" } }
```

**Rust evaluation:**

```rust
Action::Builtin(BuiltinAction { op }) => match op {
    BuiltinOp::Tag(TagOp { kind }) => Ok(json!({ "kind": kind, "value": input })),
    BuiltinOp::Identity => Ok(input),
    BuiltinOp::Merge => { /* Object.assign({}, ...input) */ },
    BuiltinOp::Flatten => { /* input.flat(1) */ },
    BuiltinOp::ExtractField(ExtractFieldOp { field }) => { /* input[field] */ },
}
```

## Complete Types

### TypeScript (serialized form)

```ts
type HandlerKind =
  | { kind: "TypeScript"; module: string; func: string;
      stepConfig?: unknown; valueSchema?: unknown }

type BuiltinOp =
  | { type: "Tag"; kind: string }
  | { type: "Identity" }
  | { type: "Merge" }
  | { type: "Flatten" }
  | { type: "ExtractField"; field: string }

type Action =
  | { kind: "Call"; handler: HandlerKind }
  | { kind: "Sequence"; actions: Action[] }
  | { kind: "Traverse"; action: Action }
  | { kind: "All"; actions: Action[] }
  | { kind: "Match"; cases: Record<string, Action> }
  | { kind: "Loop"; body: Action }
  | { kind: "Attempt"; action: Action }
  | { kind: "Builtin"; op: BuiltinOp }
  | { kind: "Step"; step: string }
```

### Rust

See `crates/barnum_ast/src/lib.rs` for the authoritative types. Every enum variant uses a named struct payload.

## Context

Global read-only environment available to all handlers. Carries API keys, workflow IDs, tenant config, or any state that should be accessible everywhere without flowing through the data pipeline.

The context is set at workflow start and passed by reference through the entire execution tree. Handlers receive it as a second argument alongside their scoped input.

**TypeScript handler interface:**

```ts
export default async function analyze(input: AnalyzeInput, context: WorkflowContext) {
    const { apiKey, runId } = context;
    // ...
}
```

**Config:**

```ts
BarnumConfig.fromConfig({
  workflow: sequence(setup, process, report),
  context: { apiKey: process.env.API_KEY, runId: crypto.randomUUID() },
});
```

**Serialized:**

```json
{
  "workflow": { "kind": "Sequence", "actions": [...] },
  "context": { "apiKey": "sk-...", "runId": "abc-123" }
}
```

**Rust Config type:**

```rust
pub struct Config {
    pub workflow: Action,
    #[serde(default)]
    pub steps: HashMap<String, Action>,
    #[serde(default)]
    pub context: Value,
}
```

The context can also be constructed and routed through the DAG topology using `All + Identity + Merge` (the user-land Reader Monad pattern), but this incurs O(N) JSON cloning per node plus 3x AST overhead. The host-level context pointer eliminates this cost entirely: one allocation, zero-cost reference passing.

## Config API

```ts
import {
  BarnumConfig, sequence, traverse, all, loop, match,
  recover, step, recur, done, call,
} from "@barnum/workflow";

// Fully anonymous workflow
BarnumConfig.fromConfig({
  workflow: sequence(
    call("./handlers.ts", "setup"),
    call("./handlers.ts", "listFiles"),
    traverse(call("./handlers.ts", "migrate")),
  ),
});

// Or with Handler imports (existing OPAQUE_HANDLER pattern)
import setupHandler from "./handlers/setup.js";
import listFilesHandler from "./handlers/list-files.js";
BarnumConfig.fromConfig({
  workflow: sequence(setupHandler, listFilesHandler),
});

// With named steps
BarnumConfig.fromConfig({
  workflow: sequence(setupHandler, step("Process")),
  steps: {
    Process: sequence(processHandler, step("Cleanup")),
    Cleanup: cleanupHandler,
  },
});
```

`fromConfig` resolves Handler objects to Call nodes, validates that every `step("X")` references a defined step, detects unreachable steps, and serializes the AST to JSON.

The `next` array on steps is eliminated. Routing is expressed in the AST. The set of reachable steps is derived by walking the AST and collecting Step references.

## Examples

### 1. Linear pipeline

Fetch, transform, report.

```ts
import fetchData from "./handlers/fetch-data.js";
import transform from "./handlers/transform.js";
import report from "./handlers/report.js";

BarnumConfig.fromConfig({
  workflow: sequence(fetchData, transform, report),
});
```

### 2. Fan-out with traverse

List files, process each in parallel.

```ts
import listFiles from "./handlers/list-files.js";
import migrate from "./handlers/migrate.js";

BarnumConfig.fromConfig({
  workflow: sequence(listFiles, traverse(migrate)),
});
```

### 3. Type-check loop

Setup, list files, migrate each, then type-check/fix until clean.

`typeCheck` returns `TypeError[]`. `classifyErrors` projects this into a discriminated union. `extractErrors` extracts the `.errors` field from the `HasErrors` variant.

```ts
import setup from "./handlers/setup.js";
import listFiles from "./handlers/list-files.js";
import migrate from "./handlers/migrate.js";
import typeCheck from "./handlers/type-check.js";
import classifyErrors from "./handlers/classify-errors.js";
import extractErrors from "./handlers/extract-errors.js";
import fix from "./handlers/fix.js";

BarnumConfig.fromConfig({
  workflow: sequence(
    setup,
    listFiles,
    traverse(migrate),
    loop(sequence(
      typeCheck,
      classifyErrors,
      match({
        HasErrors: sequence(extractErrors, traverse(fix), recur()),
        Clean: done(),
      }),
    )),
  ),
});
```

Data flow through the loop:

1. `typeCheck` receives prior output, returns `TypeError[]`.
2. `classifyErrors` receives `TypeError[]`, returns `{kind: "HasErrors", errors: [...]}` or `{kind: "Clean"}`.
3. Match routes on `kind`:
   - HasErrors: `extractErrors` returns `TypeError[]`. `traverse(fix)` maps `fix` over each. `recur()` wraps result as `{kind: "Continue", value: FixResult[]}`.
   - Clean: `done()` wraps as `{kind: "Break", value: {kind: "Clean"}}`.
4. Loop checks `kind`. Continue: feed `value` back as next iteration's input. Break: return `value`.

### 4. Parallel branches with error materialization

Fetch user data and order data in parallel. If orders fetch fails, route to a default.

```ts
import fetchUser from "./handlers/fetch-user.js";
import fetchOrders from "./handlers/fetch-orders.js";
import defaultOrders from "./handlers/default-orders.js";
import generateReport from "./handlers/generate-report.js";

BarnumConfig.fromConfig({
  workflow: sequence(
    all(
      fetchUser,
      sequence(
        attempt(fetchOrders),
        match({
          Success: extractValue,
          Failure: defaultOrders,
        }),
      ),
    ),
    generateReport,
  ),
});
```

`all` passes the same input to both branches. `attempt` wraps the result of `fetchOrders` as `{kind: "Success", value}` or `{kind: "Failure", error, input}`. Match routes explicitly. `generateReport` receives `[UserData, OrderData]`.

### 5. Recursive review loop (named steps)

Submit, review, publish or revise. The review step calls itself on rejection.

```ts
import submit from "./handlers/submit.js";
import review from "./handlers/review.js";
import classifyReview from "./handlers/classify-review.js";
import extractFeedback from "./handlers/extract-feedback.js";
import revise from "./handlers/revise.js";
import publish from "./handlers/publish.js";

BarnumConfig.fromConfig({
  workflow: sequence(submit, step("Review")),
  steps: {
    Review: sequence(
      review,
      classifyReview,
      match({
        Rejected: sequence(extractFeedback, revise, step("Review")),
        Approved: publish,
      }),
    ),
  },
});
```

Equivalent without named steps, using `loop`:

```ts
BarnumConfig.fromConfig({
  workflow: sequence(
    submit,
    loop(sequence(
      review,
      classifyReview,
      match({
        Rejected: sequence(extractFeedback, revise, recur()),
        Approved: sequence(publish, done()),
      }),
    )),
  ),
});
```

### 6. Mutual recursion (named steps required)

Writer produces content, Reviewer evaluates. Reviewer may send back to Writer. This is mutual recursion and cannot be expressed with `loop`.

```ts
import write from "./handlers/write.js";
import review from "./handlers/review.js";
import classifyReview from "./handlers/classify-review.js";
import extractFeedback from "./handlers/extract-feedback.js";
import publish from "./handlers/publish.js";

BarnumConfig.fromConfig({
  workflow: step("Writer"),
  steps: {
    Writer: sequence(write, step("Reviewer")),
    Reviewer: sequence(
      review,
      classifyReview,
      match({
        Revise: sequence(extractFeedback, step("Writer")),
        Ship: publish,
      }),
    ),
  },
});
```

## Open questions

1. **Step-level configuration.** Named steps currently have `maxRetries`, `timeout`, `maxConcurrency`. Where do these live? On the step definition alongside the action? As action wrappers (`withRetries(3, action)`)? Both?

2. **Inline computation.** Inline Bash and inline TypeScript are useful for lightweight data transformations that don't warrant a separate handler file. Deferred—every leaf is a Call for now. Can be added as an `Inline` AST node later.

3. **FlatMap convenience.** `sequence(traverse(action), flatten())` is verbose for a common pattern. A `flatMap(action)` combinator (TS-side only, compiling to Sequence + Traverse + Builtin::Flatten) would reduce boilerplate without a new AST node.

4. **Predicate convenience.** Match requires the input to be a discriminated union. Producing the union requires an explicit classification handler. A `branch(predicate, ifTrue, ifFalse)` combinator that bundles predicate evaluation + binary match would reduce boilerplate at the cost of a less general primitive.

5. **Attempt error classification.** Attempt materializes all errors into `{kind: "Failure"}`. Should the `error` field be a structured object (with `code`, `message`, `category`) rather than a flat string, so Match branches can discriminate infrastructure failures from domain errors?

7. **Handler idempotency.** Loop, Attempt, and Step enable re-execution of Call nodes. The engine provides deterministic control flow but cannot enforce pure functions. Handlers that mutate external state (database writes, file deletes) must be idempotent at the domain level, or the engine must provide at-most-once delivery guarantees via checkpointing.
