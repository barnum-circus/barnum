# Handler Config Desugaring

**Status:** Pending

**Depends on:** BUILTIN_HANDLER_KIND.md (Constant and Identity builtins)

## Motivation

`stepConfig` is a mechanism for injecting static data alongside the pipeline value. It exists as a field on the AST's `TypeScriptHandler` node and forces `createHandler` to return a callable function (so there's a call site to pass the config). Removing it from the AST and desugaring it into existing combinators eliminates a concept from both the Rust and TS sides and lets simple handlers become plain `TypedAction` values.

## Current state

### Handler creation (`handler.ts`)

`createHandler` has 4 overloads covering every combination of `inputValidator` and `stepConfigValidator`. It returns a `CallableHandler` -- a function you call (optionally with `{ stepConfig }`) to produce a `TypedAction`:

```ts
// handler.ts:65-72
export type CallableHandler<TValue, TOutput, TStepConfig> =
  ((options?: { stepConfig?: TStepConfig }) => TypedAction<TValue, TOutput>)
  & Handler<TValue, TOutput, TStepConfig>;
```

Every handler, even ones with no config, must be called: `build()`, `deploy()`, `initialize()`. The parens are pure ceremony when no config is passed.

### AST (`ast.ts`, `barnum_ast/src/lib.rs`)

TypeScript:
```ts
// ast.ts:60-66
export type TypeScriptHandler = {
  kind: "TypeScript";
  module: string;
  func: string;
  stepConfigSchema?: unknown;  // ← this goes away
  valueSchema?: unknown;       // ← this goes away (dead field, never set)
};
```

Rust:
```rust
// barnum_ast/src/lib.rs
pub struct TypeScriptHandler {
    pub module: ModulePath,
    pub func: FuncName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub step_config_schema: Option<Value>,  // ← this goes away
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_schema: Option<Value>,        // ← this goes away (dead field, never set)
}
```

### Built-in handlers that use stepConfig

Two builtins use `stepConfigValidator` to carry their parameters:

```ts
// handlers/builtins.ts
export const constant = createHandler({
  stepConfigValidator: z.object({ value: z.unknown() }),
  handle: async ({ stepConfig }) => stepConfig.value,
}, "constant");

export const range = createHandler({
  stepConfigValidator: z.object({ start: z.number(), end: z.number() }),
  handle: async ({ stepConfig }) => { ... },
}, "range");
```

These are called in `builtins.ts`:
```ts
export function constant<T>(value: T): TypedAction<never, T> {
  return constantHandler({ stepConfig: { value } }) as TypedAction<never, T>;
}
```

### Worker (`worker.ts`)

The worker already ignores `stepConfig`. It calls `handler.__definition.handle({ value: input.value })` with only the value.

### `__builtin__` module convention

Builtins like `identity()`, `tag()`, `merge()`, `flatten()`, `getField()` use `module: "__builtin__"` in their Invoke nodes. This is a synthetic path — `__builtin__` is not a real file. The scheduler would try to import it as a TypeScript module and fail. These builtins only work in the noop scheduler (tests) and are broken in real subprocess execution.

## Design

### Builtin handler kinds

See BUILTIN_HANDLER_KIND.md for the full design. The config desugaring uses two builtins:

- **Constant** (`{ kind: "Builtin", builtin: { kind: "Constant", value: <value> } }`) — returns stored value, ignores input
- **Identity** (`{ kind: "Builtin", builtin: { kind: "Identity" } }`) — returns input unchanged

These go through the normal dispatch → scheduler → complete cycle like any Invoke. The scheduler executes them inline (no subprocess). The flattener interns them in the handler pool like TypeScript handlers. No change to `FlatAction`, `FlatEntry`, or the 8-byte entry size.

### Split `createHandler` into two functions

**`createHandler`** -- handlers with no config. Returns a `TypedAction` directly (an Invoke action object). No function call needed at the workflow composition site.

```ts
// Handler definition:
export default createHandler({
  inputValidator: z.object({ artifact: z.string() }),
  handle: async ({ value }) => ({ built: true }),
});

// Workflow usage (no parens):
pipe(initialize, build, deploy, report)
```

The default export is the handler object. It IS a `TypedAction` (an Invoke node). The worker imports the module, finds `__definition.handle` on the export, and calls it.

Two overloads:
1. With `inputValidator`: `Handler<TValue, TOutput>` -- handler has typed pipeline input
2. Without: `Handler<never, TOutput>` -- handler takes no pipeline input

**`createHandlerWithConfig`** -- handlers that need static config alongside the pipeline value. Returns a function. Calling it with a config value produces a `TypedAction` containing the desugared AST.

```ts
// Handler definition:
export default createHandlerWithConfig({
  inputValidator: z.object({ artifact: z.string() }),
  stepConfigValidator: z.object({ target: z.string() }),
  handle: async ({ value, stepConfig }) => ({
    deployed: true,
    target: stepConfig.target,
  }),
});

// Workflow usage (call with config):
pipe(initialize, build, deploy({ target: "production" }), report)
```

The `handle` function receives `{ value, stepConfig }` as separate fields in the same context object.

### Desugared AST

When `deploy({ target: "production" })` is called, it produces:

```
Chain(
  All([
    Invoke(Builtin(Identity)),
    Invoke(Builtin(Constant({ target: "production" })))
  ]),
  Invoke(TypeScript(deploy))
)
```

All receives the pipeline value, passes it to both children:
- Identity builtin returns the pipeline value unchanged (scheduler executes inline, no subprocess)
- Constant builtin returns `{ target: "production" }`, ignoring its input (scheduler executes inline, no subprocess)

All collects: `[pipelineValue, { target: "production" }]`

Chain feeds this tuple to the TypeScript handler, which unpacks it via the internal wrapper.

Only the actual handler spawns a subprocess. Identity and Constant are executed inline by the scheduler.

### Internal handle wrapper

The handler's `__definition.handle` on the exported object is a wrapper that unpacks the tuple:

```ts
// What the worker calls:
handler.__definition.handle({ value: input.value })

// input.value is [pipelineValue, configValue] from the All
// The wrapper unpacks it:
internalHandle = async ({ value }) => {
  const [pipelineValue, config] = value;
  return userHandle({ value: pipelineValue, stepConfig: config });
};
```

The user writes `handle: async ({ value, stepConfig }) => ...` and the wrapper bridges the tuple representation to the two-field context. The worker is unchanged.

### Validation

For `createHandlerWithConfig`, the invoke node receives `[TValue, TStepConfig]` -- a tuple. The handler's runtime validator (if we add runtime validation later) validates both parts using the two validators composed together:

```ts
z.tuple([inputValidator, stepConfigValidator])
```

This reuses the existing validators without merging the types. The value and config remain structurally separate.

### What `createHandler` returns

An Action object with non-enumerable handler metadata:

```ts
function createHandler(definition, exportName?) {
  const filePath = getCallerFilePath();
  const funcName = exportName ?? "default";

  const action: Action = {
    kind: "Invoke",
    handler: { kind: "TypeScript", module: filePath, func: funcName },
  };

  // Non-enumerable: invisible to JSON.stringify, visible to the worker
  Object.defineProperty(action, "__definition", {
    value: definition, enumerable: false,
  });

  return action;
}
```

`JSON.stringify` skips non-enumerable properties, so the serialized config sent to Rust contains only the Invoke action. The worker imports the module and accesses `__definition` directly.

### What `createHandlerWithConfig` returns

A function with non-enumerable handler metadata:

```ts
function createHandlerWithConfig(definition, exportName?) {
  const filePath = getCallerFilePath();
  const funcName = exportName ?? "default";

  const invokeAction: Action = {
    kind: "Invoke",
    handler: { kind: "TypeScript", module: filePath, func: funcName },
  };

  // Internal handle that unpacks the [value, config] tuple
  const internalDefinition = {
    handle: async ({ value }: { value: unknown }) => {
      const [pipelineValue, config] = value as [unknown, unknown];
      return definition.handle({ value: pipelineValue, stepConfig: config });
    },
  };

  const fn = (config: TStepConfig) => {
    return {
      kind: "Chain",
      first: {
        kind: "All",
        actions: [
          { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Identity" } } },
          { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: config } } },
        ],
      },
      rest: invokeAction,
    };
  };

  Object.defineProperty(fn, "__definition", {
    value: internalDefinition, enumerable: false,
  });

  return fn;
}
```

No helper functions needed. The desugared AST uses `Identity` and `Constant` handler kinds directly — they're plain JSON objects, not handler creation calls.

### Builtin handlers

All builtins migrate to the Builtin handler kind (see BUILTIN_HANDLER_KIND.md). The config desugaring specifically requires:

- `constant(value)` → `Builtin(Constant { value })` — used in the desugared AST to inject config
- `identity()` → `Builtin(Identity)` — used in the desugared AST to pass through pipeline value
- `range(start, end)` → computed at build time, emitted as `Builtin(Constant { value: [start, ..., end-1] })`

The `constant` and `range` handler definitions in `handlers/builtins.ts` are deleted — they're no longer TypeScript handlers.

### Rust-side changes

Remove `step_config_schema` and `value_schema` from `TypeScriptHandler` (both are dead — `value_schema` is never set, `step_config_schema` stores config values that Rust never reads):

```rust
pub struct TypeScriptHandler {
    pub module: ModulePath,
    pub func: FuncName,
}
```

The `Builtin` variant on `HandlerKind` is added by BUILTIN_HANDLER_KIND.md.

The JSON schema, Zod schema, and CLI schema regenerations pick up the changes automatically via `build_schemas`.

### `invoke()` function removal

`ast.ts` exports `invoke(handler, options?)` which produces an Invoke action from a Handler reference. With `createHandler` returning a TypedAction directly and `createHandlerWithConfig` producing the desugared AST, `invoke()` has no callers. Delete it.

### `CallableHandler` type removal

No longer needed. `createHandler` returns a `TypedAction` (object), `createHandlerWithConfig` returns a function. `CallableHandler` (the intersection of function + Handler) is deleted.

### `Handler` type simplification

Drops from 3 type parameters to 2:

```ts
// Before:
export type Handler<TValue, TOutput, TStepConfig>

// After:
export type Handler<TValue, TOutput>
```

### Worker changes

None. Builtins are engine-native — the worker only handles TypeScript handlers.

### Scheduler changes

The scheduler's `dispatch` method gains a `HandlerKind::Builtin` match arm that executes builtins inline (see BUILTIN_HANDLER_KIND.md). The channel type changes from `(TaskId, Value)` to `(TaskId, Result<Value, HandlerError>)`.

## Implementation notes

When implementing, update all tests first:
- Rust test helpers (`ts_handler`, `invoke`) in `barnum_engine` and `barnum_event_loop` lose `step_config_schema` and `value_schema` fields
- Rust tests in `barnum_ast/src/flat.rs` same
- TS round-trip test snapshots lose `stepConfigSchema`/`valueSchema` fields
- TS handler tests remove `()` calls in workflow composition
- Demo `run*.ts` files remove `()` from handler references
- Add new tests for Constant and Identity handler kinds in the engine

## Changes summary

| File | Change |
|------|--------|
| `libs/barnum/src/handler.ts` | Split into `createHandler` + `createHandlerWithConfig`. Delete `CallableHandler`. Handler drops to 2 type params. |
| `libs/barnum/src/ast.ts` | Delete `invoke()`. Remove `stepConfigSchema` and `valueSchema` from `TypeScriptHandler`. Add `BuiltinHandler` to `HandlerKind` (see BUILTIN_HANDLER_KIND.md). |
| `libs/barnum/src/handlers/builtins.ts` | Delete `constant` and `range` definitions. `drop` migrates to Builtin handler kind. |
| `libs/barnum/src/builtins.ts` | All builtins emit Builtin handler kinds (see BUILTIN_HANDLER_KIND.md). |
| `libs/barnum/src/worker.ts` | No changes. |
| `libs/barnum/tests/handlers.ts` | All handlers: remove `()` calls in workflow composition. |
| `libs/barnum/tests/*.test.ts` | Update handler usage from `build()` to `build`. |
| `libs/barnum/tests/round-trip.test.ts` | Update snapshot expectations (stepConfigSchema/valueSchema fields gone). |
| `demos/simple-workflow/handlers/*.ts` | No API change for simple handlers (still `createHandler`). |
| `demos/simple-workflow/run*.ts` | Remove `()` from handler references in `pipe()`. |
| `crates/barnum_ast/src/lib.rs` | Remove `step_config_schema` and `value_schema` from `TypeScriptHandler`. Builtin handler kind changes per BUILTIN_HANDLER_KIND.md. |
| `crates/barnum_ast/src/flat.rs` | No structural changes (Builtin is a handler kind, not an action kind). Update test helpers. |
| `crates/barnum_engine/src/lib.rs` | No changes (builtins go through normal dispatch/complete). Update test helpers. |
| `crates/barnum_event_loop/src/lib.rs` | Scheduler dispatches builtins inline (see BUILTIN_HANDLER_KIND.md). Channel type changes to `Result`. |
| Regenerated schemas | `build_schemas` picks up the Rust type changes. |
