# Builtin Handler Kind

**Status:** Pending

**Depends on:** None (HANDLER_CONFIG_DESUGARING depends on this)

## Motivation

Builtins (identity, constant, drop, tag, etc.) currently use `module: "__builtin__"` in their Invoke nodes — a synthetic module path that doesn't resolve to a real file. The scheduler tries to import it as a TypeScript module and fails. These builtins only work in noop-scheduler tests and are broken in real subprocess execution.

Additionally, the handler config desugaring (HANDLER_CONFIG_DESUGARING.md) requires Constant and Identity as engine-native operations. Without them, the desugared AST would spawn subprocesses for pure structural plumbing.

## Design

### HandlerKind::Builtin

A `Builtin` variant on `HandlerKind` containing a nested `BuiltinKind` discriminated union. Each level has exactly one `kind` discriminant — no multiple discriminants on the same object.

**Rust:**
```rust
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

**TypeScript:**
```ts
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

### JSON serialization

```json
{ "kind": "Builtin", "builtin": { "kind": "Constant", "value": 42 } }
{ "kind": "Builtin", "builtin": { "kind": "Identity" } }
{ "kind": "Builtin", "builtin": { "kind": "Tag", "value": "Continue" } }
{ "kind": "Builtin", "builtin": { "kind": "ExtractField", "value": "id" } }
{ "kind": "Builtin", "builtin": { "kind": "Drop" } }
{ "kind": "Builtin", "builtin": { "kind": "Merge" } }
{ "kind": "Builtin", "builtin": { "kind": "Flatten" } }
```

Outer `kind` discriminates `HandlerKind` (TypeScript vs Builtin). Inner `kind` discriminates `BuiltinKind` (which builtin). Variants that carry data have a `value` field; variants that don't, don't.

### Builtin behaviors

| Kind | Value field | Engine behavior |
|------|-------------|-----------------|
| Constant | The value to return | Ignores input, returns `value` |
| Identity | — | Returns input unchanged |
| Drop | — | Ignores input, returns `null` |
| Tag | Kind string (e.g. `"Continue"`) | Returns `{ "kind": value, "value": input }` |
| Merge | — | Input is array of objects, returns shallow-merged object |
| Flatten | — | Input is `T[][]`, returns flattened `T[]` |
| ExtractField | Field name string (e.g. `"id"`) | Returns `input[value]` |

### Engine implementation

The Invoke arm dispatches on handler kind:

```rust
FlatAction::Invoke { handler } => {
    match self.flat_config.handler(handler) {
        HandlerKind::Builtin(builtin_handler) => {
            let result = execute_builtin(&builtin_handler.builtin, &value);
            self.deliver(parent, result)?;
        }
        HandlerKind::TypeScript(_) => {
            let task_id = self.next_task_id();
            self.task_to_parent.insert(task_id, parent);
            self.pending_dispatches.push(Dispatch { task_id, handler_id: handler, value });
        }
    }
}
```

The `execute_builtin` function matches on `BuiltinKind`:

```rust
fn execute_builtin(builtin_kind: &BuiltinKind, input: &Value) -> Value {
    match builtin_kind {
        BuiltinKind::Constant { value } => value.clone(),
        BuiltinKind::Identity => input.clone(),
        BuiltinKind::Drop => Value::Null,
        BuiltinKind::Tag { value: tag } => json!({ "kind": tag, "value": input }),
        BuiltinKind::Merge => { /* merge objects from array */ },
        BuiltinKind::Flatten => { /* flatten nested array */ },
        BuiltinKind::ExtractField { value: field } => { /* extract input[field] */ },
    }
}
```

#### advance/deliver return type mismatch

`advance` returns `Result<(), AdvanceError>` while `deliver` returns `Result<Option<Value>, CompleteError>`. This mismatch is dormant today — Invoke always creates a Dispatch, never calls `deliver`. The builtin handler kind is what surfaces it: builtins call `deliver` from within `advance` for the first time. This needs reconciliation during implementation (either change `advance`'s return type or queue immediate completions).

### TypeScript builtins

Each builtin function in `builtins.ts` emits a Builtin handler kind directly:

```ts
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
```

`range(start, end)` computes the array at config-build time and emits a Constant:

```ts
export function range(start: number, end: number): TypedAction<never, number[]> {
  const result: number[] = [];
  for (let i = start; i < end; i++) result.push(i);
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Constant", value: result } },
  } as TypedAction<never, number[]>;
}
```

### What this eliminates

- The `__builtin__` module convention — entirely gone
- The `builtin()` helper function in `builtins.ts` — replaced by direct Builtin handler kind construction
- The `constant` and `range` handler definitions in `handlers/builtins.ts` — no longer TypeScript handlers
- The `drop` handler definition in `handlers/builtins.ts` — no longer a TypeScript handler

`handlers/builtins.ts` can be deleted entirely. All builtins are engine-native.

## Implementation priority

**Phase 1** (required for config desugaring): Constant, Identity

**Phase 2** (fixes broken `__builtin__` builtins): Drop, Tag, Merge, Flatten, ExtractField

Implement as many as practical in one pass — the engine match arms are trivial for most.

## Future work

### Logging and observability

Builtins should go through the full execution loop (logging, tracing, metrics) like TypeScript handlers do. For now they're resolved inline in the engine. When observability infrastructure is added, builtins should emit events so they appear in workflow traces.

### Error handling

Engine-native builtins currently panic on invalid config or input (e.g., Merge on non-objects, Flatten on non-arrays). These should return proper errors instead. Not blocking the demo.

### Generate TypeScript types from Rust

The TypeScript AST types (`Action`, `HandlerKind`, `TypeScriptHandler`, `BuiltinHandler`, `BuiltinKind`, etc.) in `ast.ts` are manually maintained mirrors of the Rust types in `barnum_ast`. Every Rust-side change requires a corresponding manual TS edit — a maintenance burden and a source of drift.

The `build_schemas` pipeline already generates JSON Schema and Zod schemas from the Rust types. The serializable TS types in `ast.ts` should be generated from the same source rather than hand-maintained. This would mean:

- Rust types are the single source of truth for the wire format
- `BuiltinKind` variants, `HandlerKind` discriminants, `Action` variants — all derived automatically
- Adding a new builtin or action variant in Rust auto-propagates to TS
- The hand-written `ast.ts` types shrink to only the TS-specific parts (phantom types, `TypedAction`, combinators, `ConfigBuilder`) that have no Rust equivalent

This applies beyond builtins — the entire serializable AST layer (`Action`, `Config`, `HandlerKind`, `StepRef`, etc.) should be generated. The Zod schema generation is already halfway there; the missing piece is emitting plain TS types alongside the Zod validators.

## Changes summary

| File | Change |
|------|--------|
| `crates/barnum_ast/src/lib.rs` | Add `BuiltinHandler` struct, `BuiltinKind` enum, `Builtin` variant to `HandlerKind`. |
| `crates/barnum_engine/src/lib.rs` | Invoke arm dispatches on handler kind. Builtins resolved inline. Reconcile advance/deliver return types. |
| `libs/barnum/src/ast.ts` | Add `BuiltinHandler`, `BuiltinKind` types, update `HandlerKind` union. |
| `libs/barnum/src/builtins.ts` | All builtins emit Builtin handler kind. Delete `builtin()` helper. |
| `libs/barnum/src/handlers/builtins.ts` | Delete file (all builtins are engine-native). |
| `crates/barnum_ast/src/flat.rs` | No structural changes (Builtin is a handler kind, not an action kind). Update test helpers. |
| `crates/barnum_engine/src/lib.rs` tests | Update `ts_handler` helpers. Add Constant/Identity engine tests. |
| `crates/barnum_event_loop/src/lib.rs` tests | Update test helpers. |
| Regenerated schemas | `build_schemas` picks up the Rust type changes. |
