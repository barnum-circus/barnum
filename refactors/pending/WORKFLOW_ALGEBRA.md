# Workflow Algebra

## Mental Model

TypeScript is the compiler. Rust is the VM.

TypeScript combinators are AST constructors (builder pattern) that produce a JSON data structure—a program in a small DSL. JavaScript closures cannot cross the serialization boundary to Rust, so the combinators build data, not functions. The output of a workflow declaration is a JSON object.

Rust reads this AST and interprets it: dispatching to handlers, threading data between nodes, managing concurrency, and enforcing the loop protocol.

Leaf nodes reference exported functions by module path and name—the same pattern used by Temporal and Cadence for distributed execution. `fromConfig` resolves imported Handler objects into these references. See `refactors/past/OPAQUE_HANDLER.md`.

## Primitives

Ten AST node types. One leaf computation (Call). Three compositional (Sequence, Traverse, All). One routing (Match). One iteration (Loop) with two signal nodes (Continue, Break). One error handling (Recover). One named reference (Step).

Each primitive is specified in four dimensions: concept, TypeScript builder API, serialized JSON form, and Rust evaluation semantics.

The Rust evaluator has this shape:

```rust
async fn evaluate(
    action: &Action,
    input: Value,
    steps: &HashMap<String, Action>,
) -> Result<Value> {
    match action {
        // ... one arm per primitive
    }
}
```

### Call

Leaf node. The only primitive that executes external code. References an exported function in a module by path and name. The runtime loads the module and invokes the named export.

Categorically: a morphism in the Kleisli category (A → M B).

**TS builder:**

```ts
call("./handlers.ts", "analyze")

// Or via Handler import (resolved by fromConfig):
import analyzeHandler from "./handlers/analyze.js";
sequence(analyzeHandler, ...)
```

**Serialized:**

```json
{ "kind": "Call", "module": "/abs/path/handlers.ts", "func": "analyze" }
```

Optional fields `stepConfig` and `valueSchema` for TypeScript handlers (see OPAQUE_HANDLER.md).

**Rust evaluation:**

```rust
Action::Call { module, func, .. } => {
    node_runner::execute(module, func, input).await
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
        state = evaluate(action, state, steps).await?;
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
        .map(|item| evaluate(action, item.clone(), steps))
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
        .map(|action| evaluate(action, input.clone(), steps))
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
    evaluate(action, input, steps).await
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
        let result = evaluate(body, state, steps).await?;
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

#### Continue / Break

Loop signal constructors. Wrap their input in the format that Loop expects. These exist so that handlers don't need to know the loop protocol—the AST handles the wrapping.

`continue` and `break` are reserved words in TypeScript, so the builder functions are `recur()` and `done()`.

**TS builder:**

```ts
recur()   // wraps input as { kind: "Continue", value: input }
done()    // wraps input as { kind: "Break", value: input }
```

**Serialized:**

```json
{ "kind": "Continue" }
{ "kind": "Break" }
```

**Rust evaluation:**

```rust
Action::Continue => Ok(json!({ "kind": "Continue", "value": input })),
Action::Break => Ok(json!({ "kind": "Break", "value": input })),
```

### Recover

Localized error handling. Runs an action; on failure, runs a fallback.

Categorically: catchE from MonadError.

**TS builder:**

```ts
recover(action, fallback)
```

**Serialized:**

```json
{ "kind": "Recover", "action": mainAction, "fallback": fallbackAction }
```

**Rust evaluation:**

```rust
Action::Recover { action, fallback } => {
    let saved = input.clone();
    match evaluate(action, input, steps).await {
        Ok(result) => Ok(result),
        Err(error) => {
            let fallback_input = json!({
                "error": error.to_string(),
                "input": saved,
            });
            evaluate(fallback, fallback_input, steps).await
        }
    }
}
```

The fallback receives both the error and the original input.

As currently defined, Recover is a blind catch-all. It will swallow infrastructure failures (V8 OOM, module load crash, IPC timeout) alongside domain errors (expected HTTP 404, validation failure). Production implementation must distinguish these — either via an error discriminator on the Recover node or by classifying errors into kinds that Recover can filter on.

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
    evaluate(action, input, steps).await
}
```

## Complete Types

### TypeScript (serialized form)

```ts
type Action =
  | { kind: "Call"; module: string; func: string;
      stepConfig?: unknown; valueSchema?: unknown }
  | { kind: "Sequence"; actions: Action[] }
  | { kind: "Traverse"; action: Action }
  | { kind: "All"; actions: Action[] }
  | { kind: "Match"; cases: Record<string, Action> }
  | { kind: "Loop"; body: Action }
  | { kind: "Continue" }
  | { kind: "Break" }
  | { kind: "Recover"; action: Action; fallback: Action }
  | { kind: "Step"; step: string }
```

### Rust

```rust
#[derive(Debug, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind")]
pub enum Action {
    Call {
        module: String,
        func: String,
        #[serde(rename = "stepConfig")]
        step_config: Option<Value>,
        #[serde(rename = "valueSchema")]
        value_schema: Option<Value>,
    },
    Sequence { actions: Vec<Action> },
    Traverse { action: Box<Action> },
    All { actions: Vec<Action> },
    Match { cases: HashMap<String, Action> },
    Loop { body: Box<Action> },
    Continue,
    Break,
    Recover {
        action: Box<Action>,
        fallback: Box<Action>,
    },
    Step { step: String },
}
```

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

### 4. Parallel branches with error recovery

Fetch user data and order data in parallel. If orders fetch fails, use a default.

```ts
import fetchUser from "./handlers/fetch-user.js";
import fetchOrders from "./handlers/fetch-orders.js";
import defaultOrders from "./handlers/default-orders.js";
import generateReport from "./handlers/generate-report.js";

BarnumConfig.fromConfig({
  workflow: sequence(
    all(
      fetchUser,
      recover(fetchOrders, defaultOrders),
    ),
    generateReport,
  ),
});
```

`all` passes the same input to both branches. `generateReport` receives `[UserData, OrderData]`.

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

3. **FlatMap.** Traverse followed by flatten. Useful when each element produces an array and results should be concatenated. Can be expressed as `sequence(traverse(action), flattenHandler)` but a dedicated node would avoid the utility handler. Deferred.

4. **Predicate convenience.** Match requires the input to be a discriminated union. Producing the union requires an explicit classification handler. A `branch(predicate, ifTrue, ifFalse)` combinator that bundles predicate evaluation + binary match would reduce boilerplate at the cost of a less general primitive.

5. **Rust-native builtins.** A `Builtin` AST node for synchronous, FFI-free JSON transformations (`Flatten`, `ExtractField`, `Merge`, `IsNotEmpty`). These execute entirely in the Rust VM, bypassing Node IPC. Eliminates the need for trivial adapter handlers that exist only to reshape data. Also resolves FlatMap (item 3) and predicate convenience (item 4) without FFI overhead. Deferred — V1 is TypeScript-only.

6. **Recover error discrimination.** Recover currently catches all errors indiscriminately. Should accept an error filter — either a list of catchable error kinds or a predicate — so infrastructure failures (V8 OOM, IPC timeout) bypass recovery and propagate to a supervisor.

7. **Handler idempotency.** Loop, Recover, and Step enable re-execution of Call nodes. The engine provides deterministic control flow but cannot enforce pure functions. Handlers that mutate external state (database writes, file deletes) must be idempotent at the domain level, or the engine must provide at-most-once delivery guarantees via checkpointing.
