# Builtin Handler Kind

**Status:** Pending

**Depends on:** None (HANDLER_CONFIG_DESUGARING depends on this)

## Motivation

Builtins (`identity`, `constant`, `drop`, `tag`, etc.) currently use `module: "__builtin__"` in their Invoke nodes — a synthetic module path that doesn't resolve to a real file. These are broken in real subprocess execution. The handler config desugaring requires Constant and Identity as engine-native operations.

## Types

### Rust

```rust
// barnum_ast/src/lib.rs

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum HandlerKind {
    TypeScript(TypeScriptHandler),
    Builtin(BuiltinHandler),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinHandler {
    pub builtin: BuiltinKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind")]
pub enum BuiltinKind {
    Constant { value: Value },
    Identity,
    Drop,
    Tag { value: Value },
    Merge,
    Flatten,
    ExtractField { value: Value },
}
```

### TypeScript

```ts
// ast.ts

export type HandlerKind = TypeScriptHandler | BuiltinHandler;

export type BuiltinHandler = {
  kind: "Builtin";
  builtin: BuiltinKind;
};

export type BuiltinKind =
  | { kind: "Constant"; value: unknown }
  | { kind: "Identity" }
  | { kind: "Drop" }
  | { kind: "Tag"; value: string }
  | { kind: "Merge" }
  | { kind: "Flatten" }
  | { kind: "ExtractField"; value: string };
```

### JSON examples

```json
{ "kind": "Builtin", "builtin": { "kind": "Constant", "value": 42 } }
{ "kind": "Builtin", "builtin": { "kind": "Identity" } }
{ "kind": "Builtin", "builtin": { "kind": "Tag", "value": "Continue" } }
{ "kind": "Builtin", "builtin": { "kind": "ExtractField", "value": "id" } }
{ "kind": "Builtin", "builtin": { "kind": "Drop" } }
{ "kind": "Builtin", "builtin": { "kind": "Merge" } }
{ "kind": "Builtin", "builtin": { "kind": "Flatten" } }
```

Outer `kind` discriminates `HandlerKind`. Inner `kind` discriminates `BuiltinKind`. One discriminant per level.

## Engine

No changes. The engine's Invoke arm creates a Dispatch for every handler, regardless of kind. Builtins go through the same dispatch → complete cycle as TypeScript handlers.

```rust
// barnum_engine — UNCHANGED
FlatAction::Invoke { handler } => {
    let task_id = self.next_task_id();
    self.task_to_parent.insert(task_id, parent);
    self.pending_dispatches.push(Dispatch { task_id, handler_id: handler, value });
}
```

## Scheduler

The scheduler is where execution strategy is decided. Builtins are executed inline and the result is sent through the same channel as subprocess results.

```rust
// barnum_event_loop/src/lib.rs

impl Scheduler {
    pub fn dispatch(&self, dispatch: &Dispatch, handler: &HandlerKind) {
        let result_tx = self.result_tx.clone();
        let task_id = dispatch.task_id;

        match handler {
            HandlerKind::Builtin(builtin_handler) => {
                let builtin_kind = builtin_handler.builtin.clone();
                let value = dispatch.value.clone();
                tokio::spawn(async move {
                    let result = execute_builtin(&builtin_kind, &value);
                    let _ = result_tx.send((task_id, result));
                });
            }
            // Channel type: (TaskId, Result<Value, HandlerError>)
            // HandlerError covers both BuiltinError and TypeScript subprocess failures.
            // A builtin type error is the same class of failure as a TS handler throwing —
            // both propagate through the same error path in run_workflow.
            HandlerKind::TypeScript(_) => {
                match &self.mode {
                    ExecutionMode::Noop => {
                        tokio::spawn(async move {
                            let value = Value::Object(serde_json::Map::default());
                            let _ = result_tx.send((task_id, value));
                        });
                    }
                    ExecutionMode::Subprocess { executor, worker_path } => {
                        // existing subprocess logic
                    }
                }
            }
        }
    }
}
```

Builtins always execute their real logic. They're deterministic and have no external dependencies.

### Noop scheduler deprecation

The noop scheduler (`ExecutionMode::Noop`) returns `{}` for every TypeScript handler. All existing Rust tests use it — they verify structural mechanics (Chain trampolining, Parallel collection, Loop Continue/Break) but never check actual data flow.

With builtins, tests should use `Constant`, `Identity`, `Tag`, etc. as real handlers and assert on real output values. The noop mode becomes unnecessary — replace all noop-based tests with builtin-based tests that exercise real execution.

## Builtin implementations

```rust
// barnum_event_loop/src/lib.rs (or a dedicated builtins module)

#[derive(Debug, thiserror::Error)]
pub enum BuiltinError {
    #[error("{builtin}: expected {expected}, got {actual}")]
    TypeError {
        builtin: &'static str,
        expected: &'static str,
        actual: Value,
    },
}

fn execute_builtin(builtin_kind: &BuiltinKind, input: &Value) -> Result<Value, BuiltinError> {
    match builtin_kind {
        BuiltinKind::Constant { value } => Ok(value.clone()),

        BuiltinKind::Identity => Ok(input.clone()),

        BuiltinKind::Drop => Ok(Value::Null),

        BuiltinKind::Tag { value: tag } => {
            Ok(json!({ "kind": tag, "value": input }))
        }

        BuiltinKind::Merge => {
            let Value::Array(items) = input else {
                return Err(BuiltinError::TypeError {
                    builtin: "Merge",
                    expected: "array",
                    actual: input.clone(),
                });
            };
            let mut merged = serde_json::Map::new();
            for item in items {
                let Value::Object(obj) = item else {
                    return Err(BuiltinError::TypeError {
                        builtin: "Merge",
                        expected: "object in array",
                        actual: item.clone(),
                    });
                };
                for (k, v) in obj {
                    merged.insert(k.clone(), v.clone());
                }
            }
            Ok(Value::Object(merged))
        }

        BuiltinKind::Flatten => {
            let Value::Array(outer) = input else {
                return Err(BuiltinError::TypeError {
                    builtin: "Flatten",
                    expected: "array",
                    actual: input.clone(),
                });
            };
            let mut result = Vec::new();
            for item in outer {
                let Value::Array(inner) = item else {
                    return Err(BuiltinError::TypeError {
                        builtin: "Flatten",
                        expected: "array element",
                        actual: item.clone(),
                    });
                };
                result.extend(inner.iter().cloned());
            }
            Ok(Value::Array(result))
        }

        BuiltinKind::ExtractField { value: field } => {
            let Value::String(field_name) = field else {
                return Err(BuiltinError::TypeError {
                    builtin: "ExtractField",
                    expected: "string value",
                    actual: field.clone(),
                });
            };
            let Value::Object(obj) = input else {
                return Err(BuiltinError::TypeError {
                    builtin: "ExtractField",
                    expected: "object",
                    actual: input.clone(),
                });
            };
            Ok(obj.get(field_name.as_str()).cloned().unwrap_or(Value::Null))
        }
    }
}
```

## TypeScript builtins

```ts
// builtins.ts

export function constant<T>(value: T): TypedAction<never, T> {
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Constant", value } },
  } as TypedAction<never, T>;
}

export function identity<T>(): TypedAction<T, T> {
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Identity" } },
  } as TypedAction<T, T>;
}

export function drop<T>(): TypedAction<T, never> {
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Drop" } },
  } as TypedAction<T, never>;
}

export function tag<T, K extends string>(kind: K): TypedAction<T, { kind: K; value: T }> {
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Tag", value: kind } },
  } as TypedAction<T, { kind: K; value: T }>;
}

export function merge<T extends Record<string, unknown>[]>(): TypedAction<
  T,
  UnionToIntersection<T[number]>
> {
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Merge" } },
  } as TypedAction<T, UnionToIntersection<T[number]>>;
}

export function flatten<T>(): TypedAction<T[][], T[]> {
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Flatten" } },
  } as TypedAction<T[][], T[]>;
}

export function extractField<
  TObj extends Record<string, unknown>,
  TField extends keyof TObj & string,
>(field: TField): TypedAction<TObj, TObj[TField]> {
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: field } },
  } as TypedAction<TObj, TObj[TField]>;
}

export function range(start: number, end: number): TypedAction<never, number[]> {
  const result: number[] = [];
  for (let i = start; i < end; i++) result.push(i);
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Constant", value: result } },
  } as TypedAction<never, number[]>;
}

// Loop signals (unchanged API, new implementation)
export function recur(): TypedAction<any, LoopResult<any, any>> {
  return tag("Continue") as TypedAction<any, LoopResult<any, any>>;
}

export function done(): TypedAction<any, LoopResult<any, any>> {
  return tag("Break") as TypedAction<any, LoopResult<any, any>>;
}
```

## What this eliminates

- The `__builtin__` module convention
- The `builtin()` helper function in `builtins.ts`
- `handlers/builtins.ts` (delete file — all builtins are engine-native)

## Implementation priority

**Phase 1** (required for config desugaring): Constant, Identity

**Phase 2** (fixes broken builtins): Drop, Tag, Merge, Flatten, ExtractField

## Future work

### Logging and observability

Because builtins go through the scheduler, logging can be added in the scheduler's dispatch path — same hook point as TypeScript handlers.

### Delete noop execution mode

`ExecutionMode::Noop` silently stubs every handler to return `{}`. Tests that use it verify structural mechanics but never check data flow — a false sense of coverage. Delete `ExecutionMode::Noop`, delete `Scheduler::new()`, and rewrite all tests with real assertions on real values.


### Generate TypeScript types from Rust

The TypeScript AST types in `ast.ts` are manually maintained mirrors of the Rust types in `barnum_ast`. The `build_schemas` pipeline already generates JSON Schema and Zod schemas from Rust. The serializable TS types should be generated from the same source. This applies to the entire AST layer (`Action`, `Config`, `HandlerKind`, `BuiltinKind`, `StepRef`, etc.), not just builtins.

## Changes summary

| File | Change |
|------|--------|
| `crates/barnum_ast/src/lib.rs` | Add `BuiltinHandler`, `BuiltinKind`, `Builtin` variant to `HandlerKind`. |
| `crates/barnum_event_loop/src/lib.rs` | Scheduler dispatches builtins inline via `execute_builtin`. |
| `crates/barnum_engine/src/lib.rs` | No changes (builtins are normal Invoke dispatches). |
| `libs/barnum/src/ast.ts` | Add `BuiltinHandler`, `BuiltinKind` types, update `HandlerKind`. |
| `libs/barnum/src/builtins.ts` | All builtins emit Builtin handler kind. Delete `builtin()` helper. |
| `libs/barnum/src/handlers/builtins.ts` | Delete file. |
| `crates/barnum_ast/src/flat.rs` | Update test helpers only. |
| `crates/barnum_engine/src/lib.rs` tests | Update test helpers only. |
| `crates/barnum_event_loop/src/lib.rs` tests | Update test helpers, add builtin tests. |
| Regenerated schemas | `build_schemas` picks up Rust type changes. |
