# Sleep Builtin

## Motivation

`sleep(ms)` is currently implemented as a TypeScript handler (race.ts). Every sleep spawns a Node subprocess just to call `setTimeout`. This is wasteful — sleep is a scheduling primitive, not a data transformation or I/O operation. The Rust scheduler already runs inside a tokio runtime with `tokio::time::sleep` available.

The babysit-prs demo currently uses a custom `delay` handler (`createHandlerWithConfig`) that also spawns a subprocess. Both should use a Rust-native builtin.

## Design

Add `Sleep { value: Value }` to `BuiltinKind`. The `value` field holds the milliseconds as a JSON number, embedded at AST construction time. Behavior: sleep for that duration, return input unchanged (passthrough).

`execute_builtin` becomes `async fn` to support the `tokio::time::sleep` await. All other builtin variants remain synchronous (no await points), so the async overhead is negligible.

The TypeScript `sleep(ms)` function changes from constructing a TS handler invoke to constructing a `Builtin` invoke. `withTimeout` continues to use a TS handler for its dynamic-ms sleep (the duration comes from a runtime pipeline, not a build-time constant).

## Changes

### 1. `crates/barnum_ast/src/lib.rs` — Add variant

```rust
// Before (line 238-281)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum BuiltinKind {
    // ...existing variants...
    CollectSome,
}

// After
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum BuiltinKind {
    // ...existing variants...
    CollectSome,
    /// Sleep for a fixed duration, then pass input through unchanged.
    ///
    /// `value` is the duration in milliseconds (must be a non-negative integer).
    /// Unlike other builtins, execution is async — the scheduler awaits
    /// `tokio::time::sleep` before returning the input.
    Sleep {
        /// Duration in milliseconds.
        value: Value,
    },
}
```

Crate doc comment on line 1-2 says "pure data transformation" — update to note Sleep is the exception.

### 2. `crates/barnum_builtins/Cargo.toml` — Add tokio dependency

```toml
# Before
[dependencies]
barnum_ast = { path = "../barnum_ast" }
serde_json.workspace = true
thiserror.workspace = true

# After
[dependencies]
barnum_ast = { path = "../barnum_ast" }
serde_json.workspace = true
thiserror.workspace = true
tokio = { workspace = true, features = ["time"] }
```

### 3. `crates/barnum_builtins/src/lib.rs` — Make async, add Sleep

```rust
// Before (line 28-29)
#[allow(clippy::too_many_lines)]
pub fn execute_builtin(builtin_kind: &BuiltinKind, input: &Value) -> Result<Value, BuiltinError> {
    match builtin_kind {
        // ...
    }
}

// After
#[allow(clippy::too_many_lines)]
pub async fn execute_builtin(builtin_kind: &BuiltinKind, input: &Value) -> Result<Value, BuiltinError> {
    match builtin_kind {
        // ...existing arms unchanged...

        BuiltinKind::Sleep { value: ms_value } => {
            let Some(ms) = ms_value.as_u64() else {
                return Err(BuiltinError {
                    builtin: "Sleep",
                    expected: "non-negative integer milliseconds",
                    actual: ms_value.clone(),
                });
            };
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            Ok(input.clone())
        }
    }
}
```

Update crate doc comment (line 1-5):

```rust
// Before
//! Builtin handler implementations.
//!
//! Each [`BuiltinKind`] variant maps to a pure data transformation executed
//! inline by the scheduler (no subprocess). All builtins are infallible
//! except for type mismatches, which produce [`BuiltinError`].

// After
//! Builtin handler implementations.
//!
//! Each [`BuiltinKind`] variant is executed inline by the scheduler (no
//! subprocess). Most are pure data transformations. [`BuiltinKind::Sleep`]
//! is the exception — it awaits a tokio timer before returning.
//! All builtins are infallible except for type mismatches, which produce
//! [`BuiltinError`].
```

Tests: change `#[test]` to `#[tokio::test]` and add `.await` to all `execute_builtin` calls. Add Sleep tests:

```rust
#[tokio::test]
async fn sleep_passes_input_through() {
    let result = execute_builtin(
        &BuiltinKind::Sleep { value: json!(0) },
        &json!({"x": 1}),
    ).await;
    assert_eq!(result.unwrap(), json!({"x": 1}));
}

#[tokio::test]
async fn sleep_rejects_non_integer() {
    let result = execute_builtin(
        &BuiltinKind::Sleep { value: json!("bad") },
        &json!(null),
    ).await;
    assert!(result.is_err());
}
```

### 4. `crates/barnum_event_loop/src/lib.rs` — Await builtin execution

```rust
// Before (line 107-113)
HandlerKind::Builtin(builtin_handler) => {
    let builtin_kind = builtin_handler.builtin.clone();
    let value = dispatch_event.value.clone();
    tokio::spawn(async move {
        let result = execute_builtin(&builtin_kind, &value).map_err(HandlerError::from);
        let _ = result_tx.send((task_id, result));
    });
}

// After
HandlerKind::Builtin(builtin_handler) => {
    let builtin_kind = builtin_handler.builtin.clone();
    let value = dispatch_event.value.clone();
    tokio::spawn(async move {
        let result = execute_builtin(&builtin_kind, &value).await.map_err(HandlerError::from);
        let _ = result_tx.send((task_id, result));
    });
}
```

One character change: `.await` inserted before `.map_err`.

### 5. `libs/barnum/src/race.ts` — Builtin sleep, keep TS handler for withTimeout

```typescript
// Before (line 88-124)
/** The raw Invoke node for the sleep handler. */
const SLEEP_INVOKE: Action = {
  kind: "Invoke",
  handler: {
    kind: "TypeScript",
    module: import.meta.url,
    func: "sleep",
  },
};

export function sleep(): TypedAction<number, void> {
  return typedAction<number, void>(SLEEP_INVOKE);
}

Object.defineProperty(sleep, "__definition", {
  value: {
    handle: ({ value }: { value: number }) =>
      new Promise<void>((resolve) => setTimeout(resolve, value)),
  },
  enumerable: false,
});

// After

/**
 * Sleep for a fixed duration, then pass input through unchanged.
 *
 * `ms` is baked into the AST at construction time. Executed by the Rust
 * scheduler via `tokio::time::sleep` — no subprocess spawned.
 */
export function sleep<TIn = void>(ms: number): TypedAction<TIn, TIn> {
  return typedAction<TIn, TIn>({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Sleep", value: ms } },
  });
}

// --- Internal: TS handler sleep for withTimeout (dynamic ms from pipeline) ---

const DYNAMIC_SLEEP_INVOKE: Action = {
  kind: "Invoke",
  handler: {
    kind: "TypeScript",
    module: import.meta.url,
    func: "dynamicSleep",
  },
};

/** @internal TS handler that takes ms as pipeline input. Used by withTimeout. */
export function dynamicSleep(): void {}
Object.defineProperty(dynamicSleep, "__definition", {
  value: {
    handle: ({ value }: { value: number }) =>
      new Promise<void>((resolve) => setTimeout(resolve, value)),
  },
  enumerable: false,
});
```

Update `withTimeout` to use `DYNAMIC_SLEEP_INVOKE` instead of `SLEEP_INVOKE` (line 166):

```typescript
// Before (line 162-166)
    first: {
      kind: "Chain",
      first: { kind: "Chain", first: ms as Action, rest: SLEEP_INVOKE },
      rest: TAG_ERR,
    },

// After
    first: {
      kind: "Chain",
      first: { kind: "Chain", first: ms as Action, rest: DYNAMIC_SLEEP_INVOKE },
      rest: TAG_ERR,
    },
```

### 6. `demos/babysit-prs/handlers/steps.ts` — Remove delay handler

```typescript
// Before (line 1-2)
import { createHandler, createHandlerWithConfig } from "@barnum/barnum";
import { z } from "zod";

// After
import { createHandler } from "@barnum/barnum";
import { z } from "zod";
```

Delete lines 101-115 (the entire `delay` handler):

```typescript
// DELETE
// --- Delay (pass-through sleep) ---

export const delay = createHandlerWithConfig(
  {
    stepConfigValidator: z.number(),
    inputValidator: z.array(z.number()),
    outputValidator: z.array(z.number()),
    handle: async ({ value, stepConfig: ms }) => {
      console.error(`[delay] Waiting ${ms / 1000}s before retry...`);
      await new Promise((resolve) => setTimeout(resolve, ms));
      return value;
    },
  },
  "delay",
);
```

### 7. `demos/babysit-prs/run.ts` — Use builtin sleep

```typescript
// Before (line 19-27)
import {
  runPipeline,
  pipe,
  forEach,
  loop,
  drop,
  Option,
  bindInput,
} from "@barnum/barnum";
import {
  checkPR,
  fixIssues,
  landPR,
  classifyRemaining,
  delay,
} from "./handlers/steps.js";

// After
import {
  runPipeline,
  pipe,
  forEach,
  loop,
  drop,
  Option,
  bindInput,
  sleep,
} from "@barnum/barnum";
import {
  checkPR,
  fixIssues,
  landPR,
  classifyRemaining,
} from "./handlers/steps.js";
```

```typescript
// Before (line 54-57)
      classifyRemaining.branch({
        HasPRs: delay(10_000).then(recur),
        AllDone: done,
      }),

// After
      classifyRemaining.branch({
        HasPRs: sleep(10_000).then(recur),
        AllDone: done,
      }),
```

## Testing

- `cargo test -p barnum_builtins` — new Sleep tests + all existing tests (now async)
- `cargo test -p barnum_event_loop` — existing integration tests (unchanged behavior)
- `pnpm run typecheck` from repo root — verifies TS compiles
- `pnpm test` — full test suite including runPipeline integration tests
- Run babysit-prs demo manually to verify the 10s delay works end-to-end
