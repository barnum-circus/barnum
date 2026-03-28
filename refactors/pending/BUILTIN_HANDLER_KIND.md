# Builtin Handler Kind

**Status:** Pending

**Depends on:** None (HANDLER_CONFIG_DESUGARING depends on this)

## Motivation

Builtins (identity, constant, drop, tag, etc.) currently use `module: "__builtin__"` in their Invoke nodes — a synthetic module path that doesn't resolve to a real file. The scheduler tries to import it as a TypeScript module and fails. These builtins only work in noop-scheduler tests and are broken in real subprocess execution.

Additionally, the handler config desugaring (HANDLER_CONFIG_DESUGARING.md) requires Constant and Identity as engine-native operations. Without them, the desugared AST would spawn subprocesses for pure structural plumbing.

## Design

### HandlerKind::Builtin

A single `Builtin` variant on `HandlerKind` with a name discriminator and a config value:

**Rust:**
```rust
#[serde(tag = "kind")]
pub enum HandlerKind {
    TypeScript(TypeScriptHandler),
    Builtin(BuiltinHandler),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BuiltinHandler {
    pub name: BuiltinName,
    pub config: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BuiltinName {
    Constant,
    Identity,
    Drop,
    Tag,
    Merge,
    Flatten,
    ExtractField,
}
```

**TypeScript:**
```ts
export type HandlerKind = TypeScriptHandler | BuiltinHandler;

export type BuiltinHandler = {
  kind: "Builtin";
  name: BuiltinName;
  config: unknown;
};

export type BuiltinName =
  | "Constant"
  | "Identity"
  | "Drop"
  | "Tag"
  | "Merge"
  | "Flatten"
  | "ExtractField";
```

### JSON serialization

```json
{ "kind": "Builtin", "name": "Constant", "config": 42 }
{ "kind": "Builtin", "name": "Identity", "config": null }
{ "kind": "Builtin", "name": "Tag", "config": "Continue" }
{ "kind": "Builtin", "name": "ExtractField", "config": "id" }
{ "kind": "Builtin", "name": "Drop", "config": null }
{ "kind": "Builtin", "name": "Merge", "config": null }
{ "kind": "Builtin", "name": "Flatten", "config": null }
```

Config is always present. Builtins that don't need configuration use `null`.

### Why `name` is an enum, not a string

The engine must handle each builtin explicitly — there's no generic fallback. An unhandled builtin should be a compile error (exhaustive match), not a runtime panic. The enum also enumerates valid values in the JSON schema.

### Builtin behaviors

| Name | Config | Engine behavior |
|------|--------|-----------------|
| Constant | The value to return | Ignores input, returns `config` |
| Identity | `null` | Returns input unchanged |
| Drop | `null` | Ignores input, returns `null` |
| Tag | Kind string (e.g. `"Continue"`) | Returns `{ "kind": config, "value": input }` |
| Merge | `null` | Input is array of objects, returns shallow-merged object |
| Flatten | `null` | Input is `T[][]`, returns flattened `T[]` |
| ExtractField | Field name string (e.g. `"id"`) | Returns `input[config]` |

Config is never wrapped — it's the raw value. `constant(42)` serializes `config: 42`, not `config: { value: 42 }`. `tag("Continue")` serializes `config: "Continue"`, not `config: { kind: "Continue" }`.

### Engine implementation

The Invoke arm dispatches on handler kind:

```rust
FlatAction::Invoke { handler } => {
    match self.flat_config.handler(handler) {
        HandlerKind::Builtin(builtin_handler) => {
            let result = self.execute_builtin(builtin_handler, &value);
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

Note: `advance` returns `Result<(), AdvanceError>` while `deliver` returns `Result<Option<Value>, CompleteError>`. These need reconciliation — see HANDLER_CONFIG_DESUGARING.md for details.

### TypeScript builtins

Each builtin function in `builtins.ts` emits a Builtin handler kind directly:

```ts
export function constant<T>(value: T): TypedAction<never, T> {
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", name: "Constant", config: value },
  } as TypedAction<never, T>;
}

export function identity<T>(): TypedAction<T, T> {
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", name: "Identity", config: null },
  } as TypedAction<T, T>;
}

export function drop<T>(): TypedAction<T, never> {
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", name: "Drop", config: null },
  } as TypedAction<T, never>;
}

export function tag<T, K extends string>(kind: K): TypedAction<T, { kind: K; value: T }> {
  return {
    kind: "Invoke",
    handler: { kind: "Builtin", name: "Tag", config: kind },
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
    handler: { kind: "Builtin", name: "Constant", config: result },
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

The TypeScript AST types (`Action`, `HandlerKind`, `TypeScriptHandler`, `BuiltinHandler`, `BuiltinName`, etc.) in `ast.ts` are manually maintained mirrors of the Rust types in `barnum_ast`. Every Rust-side change requires a corresponding manual TS edit — a maintenance burden and a source of drift.

The `build_schemas` pipeline already generates JSON Schema and Zod schemas from the Rust types. The serializable TS types in `ast.ts` should be generated from the same source rather than hand-maintained. This would mean:

- Rust types are the single source of truth for the wire format
- `BuiltinName` variants, `HandlerKind` discriminants, `Action` variants — all derived automatically
- Adding a new builtin or action variant in Rust auto-propagates to TS
- The hand-written `ast.ts` types shrink to only the TS-specific parts (phantom types, `TypedAction`, combinators, `ConfigBuilder`) that have no Rust equivalent

This applies beyond builtins — the entire serializable AST layer (`Action`, `Config`, `HandlerKind`, `StepRef`, etc.) should be generated. The Zod schema generation is already halfway there; the missing piece is emitting plain TS types alongside the Zod validators.

## Changes summary

| File | Change |
|------|--------|
| `crates/barnum_ast/src/lib.rs` | Add `BuiltinHandler` struct, `BuiltinName` enum, `Builtin` variant to `HandlerKind`. |
| `crates/barnum_engine/src/lib.rs` | Invoke arm dispatches on handler kind. Builtins resolved inline. |
| `libs/barnum/src/ast.ts` | Add `BuiltinHandler` type, `BuiltinName` union, update `HandlerKind`. |
| `libs/barnum/src/builtins.ts` | All builtins emit Builtin handler kind. Delete `builtin()` helper. |
| `libs/barnum/src/handlers/builtins.ts` | Delete file (all builtins are engine-native). |
| `crates/barnum_ast/src/flat.rs` | No structural changes (Builtin is a handler kind, not an action kind). Update test helpers. |
| `crates/barnum_engine/src/lib.rs` tests | Update `ts_handler` helpers. Add Constant/Identity engine tests. |
| `crates/barnum_event_loop/src/lib.rs` tests | Update test helpers. |
| Regenerated schemas | `build_schemas` picks up the Rust type changes. |
