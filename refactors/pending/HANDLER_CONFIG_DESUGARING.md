# Handler Config Desugaring

**Status:** Pending

**Depends on:** None

## Motivation

`stepConfig` is a mechanism for injecting static data alongside the pipeline value. It exists as a field on the AST's `TypeScriptHandler` node and forces `createHandler` to return a callable function (so there's a call site to pass the config). Removing it from the AST and desugaring it into existing combinators eliminates a concept from both the Rust and TS sides and lets simple handlers become plain `TypedAction` values.

## Current state

### Handler creation (`handler.ts`)

`createHandler` has 4 overloads covering every combination of `stepValueValidator` and `stepConfigValidator`. It returns a `CallableHandler` -- a function you call (optionally with `{ stepConfig }`) to produce a `TypedAction`:

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
  valueSchema?: unknown;
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
    pub value_schema: Option<Value>,
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

## Design

### Split `createHandler` into two functions

**`createHandler`** -- handlers with no config. Returns a `TypedAction` directly (an Invoke action object). No function call needed at the workflow composition site.

```ts
// Handler definition:
export default createHandler({
  stepValueValidator: z.object({ artifact: z.string() }),
  handle: async ({ value }) => ({ built: true }),
});

// Workflow usage (no parens):
pipe(initialize, build, deploy, report)
```

The default export is the handler object. It IS a `TypedAction` (an Invoke node). The worker imports the module, finds `__definition.handle` on the export, and calls it.

Two overloads:
1. With `stepValueValidator`: `Handler<TValue, TOutput>` -- handler has typed pipeline input
2. Without: `Handler<never, TOutput>` -- handler takes no pipeline input

**`createHandlerWithConfig`** -- handlers that need static config alongside the pipeline value. Returns a function. Calling it with a config value produces a `TypedAction` containing the desugared AST.

```ts
// Handler definition:
export default createHandlerWithConfig({
  stepValueValidator: z.object({ artifact: z.string() }),
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
  Parallel([
    Identity,
    Chain(Drop, Constant({ target: "production" }))
  ]),
  Invoke(deploy)
)
```

In words: take the pipeline value, pair it with the constant config via Parallel, then invoke the handler. The handler receives `[pipelineValue, config]` as a two-element array (the output of Parallel).

### Internal handle wrapper

The handler's `__definition.handle` on the exported object is a wrapper that unpacks the tuple:

```ts
// What the worker calls:
handler.__definition.handle({ value: input.value })

// input.value is [pipelineValue, configValue] from the Parallel
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
z.tuple([stepValueValidator, stepConfigValidator])
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
        kind: "Parallel",
        actions: [
          identityAction(),
          { kind: "Chain", first: dropAction(), rest: constantAction(config) },
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

The AST helper functions (`identityAction`, `dropAction`, `constantAction`) construct raw Action objects inline to avoid circular imports with the combinator modules.

### Builtin handlers

`constant` and `range` currently use `stepConfigValidator` to carry their parameters. After this change, they use `createHandlerWithConfig`:

```ts
export const constant = createHandlerWithConfig({
  stepConfigValidator: z.object({ value: z.unknown() }),
  handle: async ({ stepConfig }) => stepConfig.value,
}, "constant");

export const range = createHandlerWithConfig({
  stepConfigValidator: z.object({ start: z.number(), end: z.number() }),
  handle: async ({ stepConfig }) => { ... },
}, "range");
```

The `constant()` and `range()` wrappers in `builtins.ts` call these with the config:

```ts
export function constant<T>(value: T): TypedAction<never, T> {
  return constantHandler({ value }) as TypedAction<never, T>;
}
```

Note: this means `constant(x)` itself emits a `Chain(Parallel([Identity, Chain(Drop, Constant(x))]), Invoke)` subtree, which is recursive. The innermost `Constant` is a raw `__builtin__` invoke, not the desugared version. The desugaring only applies at the `createHandlerWithConfig` boundary.

Actually, this recursion is a problem: the `constantAction()` helper used inside `createHandlerWithConfig` would reference the `constant` builtin, which itself is a `createHandlerWithConfig` result. We need the raw builtin `constant` to remain a simple `__builtin__` invoke that the worker handles directly, separate from the user-facing `constant()` combinator.

The fix: `constant` and `range` in `handlers/builtins.ts` stay as `createHandler` (no config), and their parameters are encoded in the `func` field (like `tag` and `extractField` already do):

```ts
// handlers/builtins.ts -- these are raw builtins, not config handlers
export const drop = createHandler({
  handle: async () => undefined,
}, "drop");

// constant and range parameters are embedded in the func name at the builtin() call site
// They don't need createHandlerWithConfig -- they're internal plumbing
```

The `builtins.ts` wrappers continue using the `builtin()` helper:

```ts
function builtin(func: string): TypedAction<any, any> {
  return {
    kind: "Invoke",
    handler: { kind: "TypeScript", module: "__builtin__", func },
  };
}

export function constant<T>(value: T): TypedAction<never, T> {
  // Encode the constant value in the func field
  return builtin(`constant:${JSON.stringify(value)}`);
}
```

The worker's builtin dispatch already handles `constant:...` by parsing the value from the func name. This keeps the internal plumbing separate from the user-facing `createHandlerWithConfig`.

### Rust-side changes

Remove `step_config_schema` from `TypeScriptHandler`:

```rust
pub struct TypeScriptHandler {
    pub module: ModulePath,
    pub func: FuncName,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_schema: Option<Value>,
}
```

The engine, flattener, and all Rust code that touches `step_config_schema` lose that field. This is a straightforward deletion since Rust never interprets the value -- it only serializes it back out for the worker, which already ignores it.

The JSON schema, Zod schema, and CLI schema regenerations pick up the change automatically via `build_schemas`.

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

None. The worker calls `handler.__definition.handle({ value: input.value })`. For simple handlers, `input.value` is the pipeline value. For config handlers, `input.value` is `[pipelineValue, config]` and the internal wrapper unpacks it. Same code path either way.

## Changes summary

| File | Change |
|------|--------|
| `libs/barnum/src/handler.ts` | Split into `createHandler` + `createHandlerWithConfig`. Delete `CallableHandler`. Handler drops to 2 type params. |
| `libs/barnum/src/ast.ts` | Delete `invoke()`. Remove `stepConfigSchema` from `TypeScriptHandler`. |
| `libs/barnum/src/handlers/builtins.ts` | `constant` and `range` become `createHandler` (encode params in func name, not stepConfig). `drop` unchanged. |
| `libs/barnum/src/builtins.ts` | `constant()` wrapper uses `builtin("constant:${JSON.stringify(value)}")` instead of calling handler with stepConfig. `range()` similar. |
| `libs/barnum/src/worker.ts` | No change. |
| `libs/barnum/tests/handlers.ts` | All handlers: remove `()` calls in workflow composition. |
| `libs/barnum/tests/*.test.ts` | Update handler usage from `build()` to `build`. |
| `demos/simple-workflow/handlers/*.ts` | Update `createHandler` calls (no API change for simple handlers). |
| `demos/simple-workflow/run*.ts` | Remove `()` from handler references in `pipe()`. |
| `crates/barnum_ast/src/lib.rs` | Remove `step_config_schema` from `TypeScriptHandler`. |
| `crates/barnum_ast/src/flat.rs` | Remove `step_config_schema` from `FlatAction` if present. |
| Regenerated schemas | `build_schemas` picks up the Rust type change. |
