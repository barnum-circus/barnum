# Trapeze Implementation Plan

Master planning document for the workflow algebra engine rewrite.

## Architecture Overview

Two runtimes, one protocol. TypeScript constructs the AST (the "compiler"), Rust interprets it (the "VM"). The serialization boundary is JSON.

### Crate structure

```
crates/
  barnum_engine/     # AST types, deserialization, config schema
  barnum_cli/        # CLI binary: `barnum run`, `barnum validate`, etc.
```

`barnum_engine` owns the AST enum, the top-level `Config` struct, and serde deserialization. `barnum_cli` depends on `barnum_engine` and exposes subcommands.

### TypeScript structure

```
libs/barnum/
  src/
    core.ts          # AST constructors: call, sequence, traverse, all, match, loop, recur, done, recover, step
    combinators.ts   # User-land combinators: identity, preserve, accumulate
    handler.ts       # createHandler, Handler class, isHandler
    builtins.ts      # Built-in handlers: identity, merge (JS for now, Rust-native later)
    config.ts        # BarnumConfig.fromConfig — resolves Handlers, serializes AST
  index.ts           # Public API re-exports
```

### Config format

The serialized config that crosses the JS-to-Rust boundary:

```json
{
  "workflow": { "kind": "Sequence", "actions": [...] },
  "steps": {
    "Review": { "kind": "Sequence", "actions": [...] }
  }
}
```

Rust types:

```rust
#[derive(Debug, Deserialize)]
pub struct Config {
    pub workflow: Action,
    #[serde(default)]
    pub steps: HashMap<String, Action>,
}
```

The `workflow` field is the entry point. `steps` is a map of named actions for mutual recursion via `Step` nodes. `fromConfig` resolves Handler objects into Call nodes and validates step references before serializing.

## Deferred: Multi-Language Handlers

V1 handlers are TypeScript only. The `Call` node references a `.ts` module and export name. The Rust runtime spawns a Node.js process to execute the handler.

Future handler languages (Bash, Python, Rust-native builtins) will extend the `Call` node with a `language` discriminator or introduce a parallel `Builtin` AST node for FFI-free execution. This is out of scope for the initial implementation.

## Deferred: Inline Handlers

Inline computation (anonymous TypeScript/Bash embedded directly in the AST) would be convenient for lightweight transformations. Deferred because every leaf being a `Call` to a named export keeps the architecture simple and testable. Can be added later as an `Inline` AST variant.

## Implementation Tasks

### Task 1: AST types in `barnum_engine`

Create the `barnum_engine` crate with the `Action` enum and `Config` struct. Both implement `Deserialize`. This is the core data model from WORKFLOW_ALGEBRA.md.

**Crate:** `crates/barnum_engine/`

**Types to define:**

```rust
use std::collections::HashMap;
use serde::Deserialize;
use serde_json::Value;

#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind")]
pub enum Action {
    Call {
        module: String,
        func: String,
        #[serde(default, rename = "stepConfig")]
        step_config: Option<Value>,
        #[serde(default, rename = "valueSchema")]
        value_schema: Option<Value>,
    },
    Sequence {
        actions: Vec<Action>,
    },
    Traverse {
        action: Box<Action>,
    },
    All {
        actions: Vec<Action>,
    },
    Match {
        cases: HashMap<String, Action>,
    },
    Loop {
        body: Box<Action>,
    },
    Continue,
    Break,
    Recover {
        action: Box<Action>,
        fallback: Box<Action>,
    },
    Step {
        step: String,
    },
}

#[derive(Debug, Clone, Deserialize)]
pub struct Config {
    pub workflow: Action,
    #[serde(default)]
    pub steps: HashMap<String, Action>,
}
```

**Test:** Deserialize a JSON string containing every variant and verify the resulting struct.

### Task 2: `barnum run` subcommand

Wire up `barnum_cli` to accept a `run` subcommand. `barnum run` reads a JSON config from stdin, deserializes it into a `Config`, and prints the debug representation.

This is the minimal proof that the AST round-trips through serde correctly. No execution, no handler invocation.

**Behavior:**

```bash
echo '{"workflow": {"kind": "Call", "module": "./h.ts", "func": "greet"}}' | barnum run
# Prints the deserialized Config struct (Debug format or re-serialized JSON)
```

**CLI structure (clap):**

```rust
#[derive(Parser)]
enum Cli {
    Run(RunArgs),
}

#[derive(Args)]
struct RunArgs {
    // Config comes from stdin for now
}
```

Read stdin, deserialize with `serde_json::from_reader`, print the result. Exit 0 on success, exit 1 with error message on deserialization failure.

### Task 3: TypeScript core constructors

One file (`src/core.ts`) with pure functions that produce AST node objects. No side effects, no I/O. Each function returns a plain object matching the serialized JSON schema.

```ts
// The Action type — discriminated union matching Rust's Action enum
export type Action = /* ... all variants from WORKFLOW_ALGEBRA.md ... */

export function call(module: string, func: string): Action
export function sequence(...actions: Action[]): Action
export function traverse(action: Action): Action
export function all(...actions: Action[]): Action
// match is a reserved-ish word, use matchCases or similar
export function matchCases(cases: Record<string, Action>): Action
export function loop(body: Action): Action
export function recur(): Action   // produces Continue node
export function done(): Action    // produces Break node
export function recover(action: Action, fallback: Action): Action
export function step(name: string): Action
```

Each function is a one-liner that returns the corresponding `{ kind: "...", ... }` object.

### Task 4: TypeScript user-land combinators

One file (`src/combinators.ts`) with higher-level combinators built from core constructors. These are convenience functions, not new AST nodes.

```ts
import { all, sequence } from "./core.js";

// Identity: passes input through unchanged
export function identity(): Action  // implemented as a built-in handler Call

// Preserve: run an action but keep the original input available
// sequence(all(identity(), action), merge())
export function preserve(action: Action): Action

// Accumulate: run a sequence of actions, threading each result
// into an accumulating object
export function accumulate(...actions: Action[]): Action
```

The exact set of combinators depends on what the built-in handlers expose. `identity()` and `preserve()` are the essentials from the Arrow problem analysis.

### Task 5: `createHandler` and `Handler` class

One file (`src/handler.ts`) implementing the opaque handler pattern from OPAQUE_HANDLER.md (adapted for the new algebra).

Core pieces:
- `Handler` class with branded symbol, `__filePath`, `__definition`
- `createHandler(definition)` — captures caller file path from stack trace
- `isHandler(x)` — brand check
- `getCallerFilePath()` — V8 `Error.prepareStackTrace` API

The handler definition interface changes from the old Barnum shape. In the new algebra, a handler is simpler:

```ts
export interface HandlerDefinition<In = unknown, Out = unknown> {
  handle: (input: In) => Promise<Out>;
  // Optional: validators for input/output
  inputSchema?: unknown;
  outputSchema?: unknown;
}
```

The old `stepConfigValidator` / `getStepValueValidator` / `FollowUpTask` pattern is gone. Handlers are pure functions from input to output. Control flow lives in the AST, not in handler return values.

### Task 6: Built-in handlers

One file (`src/builtins.ts`). These are handlers that will eventually be Rust-native builtins executing without FFI, but for V1 they are plain TypeScript handlers.

```ts
import { createHandler } from "./handler.js";

// Returns input unchanged
export const identityHandler = createHandler({ handle: async (input) => input });

// Merges an array of objects into a single object
// [{ a: 1 }, { b: 2 }] => { a: 1, b: 2 }
export const mergeHandler = createHandler({ handle: async (input) => Object.assign({}, ...input) });

// Extracts a field from an object
// Used via stepConfig: { field: "errors" }
// { errors: [...], other: ... } => [...]
export const extractFieldHandler = createHandler({
  handle: async (input, config) => input[config.field],
});
```

The exact set will be driven by what the examples and combinators need.

### Task 7: `BarnumConfig.fromConfig`

One file (`src/config.ts`). This is the user-facing entry point. It takes a config object (with Handler imports and AST combinators), resolves handlers into Call nodes, validates step references, and produces the serialized JSON that goes to the Rust binary.

```ts
export class BarnumConfig {
  static fromConfig(config: ConfigInput): BarnumConfig {
    const resolved = resolveHandlers(config);
    validateStepReferences(resolved);
    return new BarnumConfig(resolved);
  }

  run(): Promise<void> {
    const json = JSON.stringify(this.config);
    // Spawn barnum binary, pipe json to stdin, read stdout
  }
}
```

`resolveHandlers` walks the AST recursively. When it encounters a Handler object (detected via `isHandler`), it replaces it with a `Call` node pointing to the handler's file path. This is a recursive AST transform, unlike the old flat step-list walk.

`validateStepReferences` walks the AST, collects all `Step` node references, and verifies each one exists in the `steps` map. Also detects unreachable steps.

### Task 8: Unit tests

TypeScript tests that construct workflows using the builder API, call `fromConfig`, invoke `barnum run` as a subprocess, and assert the JSON output.

Test structure:

```ts
import { sequence, call, traverse, loop, recur, done, matchCases } from "@barnum/barnum";

test("linear sequence deserializes correctly", () => {
  const config = BarnumConfig.fromConfig({
    workflow: sequence(
      call("./handlers.ts", "setup"),
      call("./handlers.ts", "process"),
    ),
  });
  const result = await config.run(); // invokes binary, captures stdout
  // Assert the output matches expected deserialized structure
});
```

Tests cover:
- Every AST node type in isolation
- Nested compositions (sequence of traverse, loop containing match, etc.)
- Handler resolution (Handler objects become Call nodes)
- Step reference validation (valid refs pass, dangling refs fail)
- Round-trip fidelity (TS builder -> JSON -> Rust deserialize -> JSON output matches)

These are integration tests in the sense that they spawn the binary, but they test deserialization, not execution. Execution tests come later when the evaluator exists.

## Execution Engine (Future)

After the AST and serialization layer are solid, the evaluator from WORKFLOW_ALGEBRA.md gets implemented in `barnum_engine`. The `evaluate` function walks the AST recursively, dispatching Call nodes to handler processes and threading values through the compositional nodes.

This is a separate phase because it requires:
- Handler process management (spawn Node, pipe JSON, read result)
- Concurrency control (buffer_unordered for Traverse)
- Error propagation (Recover)
- Loop protocol enforcement (Continue/Break signal validation)

Each of these is a separate implementation task.
