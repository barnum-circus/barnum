# Workflow Algebra

## Motivation

Handlers today return `[{kind: "StepName", value: ...}]`, coupling them to the graph topology of the calling workflow. A reusable `ts-check` handler can't be shared between workflows that route differently.

Handlers should return plain values. Routing, composition, and control flow are expressed as an AST. The combinators are AST constructors (builder pattern) that produce a JSON data structure, not closures. JavaScript closures cannot cross the serialization boundary to Rust. The output of a workflow declaration is a JSON object.

Leaf nodes reference exported functions by module path and name, the same pattern used by Temporal and Cadence for distributed execution. `fromConfig` resolves imported Handler objects into these references. See `refactors/past/OPAQUE_HANDLER.md`.

## Primitives

Twelve AST node types. Six are compositional (Sequence, Map, FlatMap, All, Match, Loop). Two are leaf computations (Call, Inline). Two are loop signals (Recur, Done). One handles errors (Recover). One provides named references (Step).

Each primitive is specified in three dimensions: the TypeScript builder API, the serialized JSON form, and the Rust evaluation semantics.

### Call

Reference to an exported function in a module. The runtime loads the module and invokes the named export.

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

Load module, invoke `func` with input. For `.ts` modules: V8 isolate or bridge script. For `.sh` modules: spawn bash. Return stdout as JSON.

### Inline

Inline source code. A string, not a closure. Serializes trivially.

**TS builder:**

```ts
bash(`jq '.errors'`)
ts(`(input) => input.errors`)
```

**Serialized:**

```json
{ "kind": "Inline", "language": "Bash", "source": "jq '.errors'" }
{ "kind": "Inline", "language": "TypeScript", "source": "(input) => input.errors" }
```

**Rust evaluation:**

Bash: `bash -c <source>` with input on stdin, capture stdout. TypeScript: evaluate source in V8 isolate with input as argument.

### Sequence

Sequential composition (Kleisli composition). Each action receives the previous action's output.

**TS builder:**

```ts
sequence(a, b, c)
```

**Serialized:**

```json
{ "kind": "Sequence", "actions": [a, b, c] }
```

**Rust evaluation:**

```
let data = input
for action in actions:
  data = evaluate(action, data)
return data
```

Failure at any point aborts the sequence.

### Map

Parallel map over an array. Applies an action to each element concurrently, preserving order. Categorically: traverse over List with the Promise applicative.

**TS builder:**

```ts
map(action)
```

**Serialized:**

```json
{ "kind": "Map", "action": action }
```

**Rust evaluation:**

```
let items = parse_array(input)
let results = parallel_map(items, |item| evaluate(action, item))
return json_array(results)
```

Input must be a JSON array.

### FlatMap

Like Map, but the action returns an array per element. Results are concatenated.

**TS builder:**

```ts
flatMap(action)
```

**Serialized:**

```json
{ "kind": "FlatMap", "action": action }
```

**Rust evaluation:**

Same as Map, then flatten the resulting array of arrays into a single array.

### All

Passes the same input to multiple independent actions in parallel. Collects results as a tuple (JSON array). Categorically: applicative zip / arrow fanout.

**TS builder:**

```ts
all(a, b, c)
```

**Serialized:**

```json
{ "kind": "All", "actions": [a, b, c] }
```

**Rust evaluation:**

```
let results = parallel_map(actions, |action| evaluate(action, input.clone()))
return json_array(results)
```

Each action receives the same input.

### Match

N-ary coproduct eliminator. Routes execution based on the `kind` field of a discriminated union. The cases map provides a handler for each variant. TypeScript's type system enforces exhaustive handling via distributive conditional types over the union's `kind` literals.

**TS builder:**

```ts
match({
  HasErrors: handleErrorsAction,
  Clean: handleCleanAction,
})
```

The type signature enforces exhaustiveness:

```ts
type MatchCases<U extends { kind: string }, Out> = {
  [K in U['kind']]: Action  // must cover every variant
};
```

**Serialized:**

```json
{
  "kind": "Match",
  "cases": {
    "HasErrors": { ... action ... },
    "Clean": { ... action ... }
  }
}
```

**Rust evaluation:**

```
let variant_kind = input["kind"].as_str()
let handler = cases[variant_kind]
return evaluate(handler, input)
```

The handler receives the full variant object (including its `kind` field). If the `kind` value has no matching case, evaluation fails.

### Loop

Repeatedly executes a body action. The body must produce output with `kind: "Recur"` (continue with new state) or `kind: "Done"` (exit with result). Both carry a `value` field containing the payload. Categorically: tailRecM (monadic fixed-point).

**TS builder:**

```ts
loop(body)
```

**Serialized:**

```json
{ "kind": "Loop", "body": bodyAction }
```

**Rust evaluation:**

```
let state = input
loop:
  let result = evaluate(body, state)
  match result["kind"]:
    "Recur" => state = result["value"]; continue
    "Done"  => return result["value"]
```

The loop's initial state is its pipeline input. `Recur`'s value becomes the next iteration's input. `Done`'s value is the loop's output.

### Recur / Done

Loop signal constructors. Wrap their input in the tagged format that Loop expects.

**TS builder:**

```ts
recur()   // wraps input as { kind: "Recur", value: input }
done()    // wraps input as { kind: "Done", value: input }
```

**Serialized:**

```json
{ "kind": "Recur" }
{ "kind": "Done" }
```

**Rust evaluation:**

```
// Recur
return { "kind": "Recur", "value": input }

// Done
return { "kind": "Done", "value": input }
```

### Recover

Localized error handling. Runs an action; on failure, runs a fallback. Categorically: catchE from MonadError.

**TS builder:**

```ts
recover(action, fallback)
```

**Serialized:**

```json
{ "kind": "Recover", "action": mainAction, "fallback": fallbackAction }
```

**Rust evaluation:**

```
let saved = input.clone()
match evaluate(action, input):
  Ok(result) => return result
  Err(error) => return evaluate(fallback, { "error": error, "input": saved })
```

The fallback receives both the error and the original input.

### Step

Invokes a named step. Dispatches the current value to the step's action, waits for it and all recursive descendants. Returns the result. Named steps are Kleisli arrows with names, callable from multiple points in the AST.

**TS builder:**

```ts
step("TypeCheck")
```

**Serialized:**

```json
{ "kind": "Step", "step": "TypeCheck" }
```

**Rust evaluation:**

```
let step_action = steps[step_name]
return evaluate(step_action, input)
```

## Complete types

### TypeScript (serialized form)

```ts
type Action =
  | { kind: "Call"; module: string; func: string;
      stepConfig?: unknown; valueSchema?: unknown }
  | { kind: "Inline"; language: "Bash" | "TypeScript"; source: string }
  | { kind: "Sequence"; actions: Action[] }
  | { kind: "Map"; action: Action }
  | { kind: "FlatMap"; action: Action }
  | { kind: "All"; actions: Action[] }
  | { kind: "Match"; cases: Record<string, Action> }
  | { kind: "Loop"; body: Action }
  | { kind: "Recur" }
  | { kind: "Done" }
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
        step_config: Option<Value>,
        value_schema: Option<Value>,
    },
    Inline {
        language: Language,
        source: String,
    },
    Sequence { actions: Vec<Action> },
    Map { action: Box<Action> },
    FlatMap { action: Box<Action> },
    All { actions: Vec<Action> },
    Match { cases: HashMap<String, Action> },
    Loop { body: Box<Action> },
    Recur,
    Done,
    Recover {
        action: Box<Action>,
        fallback: Box<Action>,
    },
    Step { step: String },
}

#[derive(Debug, Serialize, Deserialize, JsonSchema)]
pub enum Language {
    Bash,
    TypeScript,
}
```

## Config API

```ts
import {
  BarnumConfig, sequence, map, flatMap, loop, match, all,
  recover, step, recur, done, bash, call,
} from "@barnum/workflow";

// Fully anonymous workflow
BarnumConfig.fromConfig({
  workflow: sequence(
    call("./handlers.ts", "setup"),
    call("./handlers.ts", "listFiles"),
    map(call("./handlers.ts", "migrate")),
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

### 1. Linear pipeline (fully anonymous)

Fetch, transform, report.

```ts
import { sequence, bash } from "@barnum/workflow";
import fetchHandler from "./handlers/fetch.js";
import reportHandler from "./handlers/report.js";

BarnumConfig.fromConfig({
  workflow: sequence(
    fetchHandler,
    bash(`jq '{summary: .data | length, items: .data}'`),
    reportHandler,
  ),
});
```

### 2. Fan-out with map (fully anonymous)

List files, process each in parallel.

```ts
import { sequence, map } from "@barnum/workflow";
import listFilesHandler from "./handlers/list-files.js";
import migrateHandler from "./handlers/migrate.js";

BarnumConfig.fromConfig({
  workflow: sequence(
    listFilesHandler,
    map(migrateHandler),
  ),
});
```

### 3. Type-check loop (fully anonymous)

Setup, list files, migrate each, then type-check/fix until clean. This is the motivating example from the design discussion.

`typeCheckHandler` returns `TypeError[]`. The classification step projects this into a discriminated union for the match.

```ts
import { sequence, map, loop, match, recur, done, bash } from "@barnum/workflow";
import setupHandler from "./handlers/setup.js";
import listFilesHandler from "./handlers/list-files.js";
import migrateHandler from "./handlers/migrate.js";
import typeCheckHandler from "./handlers/type-check.js";
import fixHandler from "./handlers/fix.js";

const classifyErrors = bash(
  `jq 'if length > 0 then {kind: "HasErrors", errors: .} else {kind: "Clean"} end'`
);

BarnumConfig.fromConfig({
  workflow: sequence(
    setupHandler,
    listFilesHandler,
    map(migrateHandler),
    loop(sequence(
      typeCheckHandler,
      classifyErrors,
      match({
        HasErrors: sequence(bash(`jq '.errors'`), map(fixHandler), recur()),
        Clean: done(),
      }),
    )),
  ),
});
```

Data flow through the loop:

1. `typeCheckHandler` receives `MigrateResult[]` (ignores), returns `TypeError[]`.
2. `classifyErrors` wraps as `{kind: "HasErrors", errors: [...]}` or `{kind: "Clean"}`.
3. Match routes on `kind`:
   - HasErrors: extract `.errors` array, map `fixHandler` over each, `recur()` wraps result as `{kind: "Recur", value: FixResult[]}`.
   - Clean: `done()` wraps as `{kind: "Done", value: {kind: "Clean"}}`.
4. Loop checks output `kind`. Recur: feed `value` back as next iteration's input. Done: return `value`.

### 4. Parallel branches with error recovery (fully anonymous)

Fetch user data and order data in parallel. If orders fetch fails, use a default.

```ts
import { sequence, all, recover, bash } from "@barnum/workflow";
import fetchUser from "./handlers/fetch-user.js";
import fetchOrders from "./handlers/fetch-orders.js";
import generateReport from "./handlers/generate-report.js";

BarnumConfig.fromConfig({
  workflow: sequence(
    all(
      fetchUser,
      recover(fetchOrders, bash(`jq '{orders: [], error: .error}'`)),
    ),
    generateReport,
  ),
});
```

`all` passes the same input to both branches. `generateReport` receives `[UserData, OrderData]`.

### 5. Recursive review loop (named steps)

Submit, review, publish or revise. The review step calls itself on rejection.

```ts
import { sequence, step, match, bash } from "@barnum/workflow";
import submitHandler from "./handlers/submit.js";
import reviewHandler from "./handlers/review.js";
import reviseHandler from "./handlers/revise.js";
import publishHandler from "./handlers/publish.js";

const classifyReview = bash(
  `jq 'if .approved then {kind: "Approved"} else {kind: "Rejected", feedback: .feedback} end'`
);

BarnumConfig.fromConfig({
  workflow: sequence(submitHandler, step("Review")),
  steps: {
    Review: sequence(
      reviewHandler,
      classifyReview,
      match({
        Rejected: sequence(bash(`jq '.feedback'`), reviseHandler, step("Review")),
        Approved: publishHandler,
      }),
    ),
  },
});
```

Equivalent without named steps, using `loop`:

```ts
BarnumConfig.fromConfig({
  workflow: sequence(
    submitHandler,
    loop(sequence(
      reviewHandler,
      classifyReview,
      match({
        Rejected: sequence(bash(`jq '.feedback'`), reviseHandler, recur()),
        Approved: sequence(publishHandler, done()),
      }),
    )),
  ),
});
```

### 6. Mutual recursion (named steps required)

Writer produces content, Reviewer evaluates. Reviewer may send back to Writer. This is mutual recursion and cannot be expressed with `loop`.

```ts
import { sequence, step, match, bash } from "@barnum/workflow";
import writeHandler from "./handlers/write.js";
import reviewHandler from "./handlers/review.js";
import publishHandler from "./handlers/publish.js";

const classifyReview = bash(
  `jq 'if .needsRevision then {kind: "Revise", feedback: .feedback} else {kind: "Ship", content: .content} end'`
);

BarnumConfig.fromConfig({
  workflow: step("Writer"),
  steps: {
    Writer: sequence(writeHandler, step("Reviewer")),
    Reviewer: sequence(
      reviewHandler,
      classifyReview,
      match({
        Revise: sequence(bash(`jq '.feedback'`), step("Writer")),
        Ship: publishHandler,
      }),
    ),
  },
});
```

## Open questions

1. **Step-level configuration.** Named steps currently have `maxRetries`, `timeout`, `maxConcurrency`. Where do these live? On the step definition alongside the action? As action wrappers (`withRetries(3, action)`)? Both?

2. **Predicate convenience.** Match requires the input to be a discriminated union. Producing the union requires an explicit classification step (like `classifyErrors`). A `branch(predicate, ifTrue, ifFalse)` combinator that bundles predicate evaluation + binary match would reduce boilerplate at the cost of categorical purity.

3. **Handlers returning discriminated unions directly.** If a handler's return type is a Zod discriminated union, `fromConfig` could derive the match cases from the schema automatically. The handler already knows its possible outputs; the config shouldn't need to repeat them.
