# Handler Config as Userland Desugaring

Speculation on whether handler configs (`stepConfig`) need to be a fundamental concept in the Rust AST, or whether the TS layer can compile them away.

## Current state

A handler receives `{ value, stepConfig }`. The config is a static value baked into the AST node at build time:

```ts
// TS surface syntax:
setup({ stepConfig: { timeout: 5000 } })

// Serialized AST node:
{
  "kind": "Invoke",
  "handler": {
    "kind": "TypeScript",
    "module": "/app/handlers/setup.ts",
    "func": "default",
    "stepConfigSchema": { "timeout": 5000 }
  }
}
```

Rust carries `step_config_schema: Option<Value>` on `TypeScriptHandler`. It doesn't interpret it — just passes it through to the TS runtime, which feeds it to the handler as `stepConfig`.

## The proposal

Handlers always take a single `value`. No `stepConfig` field. The TS `invoke()` / callable handler syntax desugars config into AST nodes:

```ts
// Surface syntax (unchanged):
setup({ stepConfig: { timeout: 5000 } })

// Desugared AST (what actually gets serialized):
Chain(
  Parallel([identity(), Chain(drop(), constant({ timeout: 5000 }))]),
  Invoke(setup)   // handler receives [pipelineValue, { timeout: 5000 }]
)
```

The handler's runtime input becomes the tuple `[value, config]` instead of `{ value, stepConfig }`. Or if we prefer objects, the Parallel + merge pattern:

```ts
Chain(
  Parallel([
    Chain(identity(), tag("value")),
    Chain(drop(), constant({ timeout: 5000 }), tag("stepConfig")),
  ]),
  merge(),
  Invoke(setup)   // handler receives { value: pipelineValue, stepConfig: { timeout: 5000 } }
)
```

This preserves the exact same handler input shape without the Rust side knowing about step configs.

## What changes

- **Rust AST**: Remove `step_config_schema` from `TypeScriptHandler`. `HandlerKind` becomes just `{ module, func }`.
- **Rust engine**: No awareness of configs. Just passes values through actions.
- **TS `invoke()` / callable handler**: When config is provided, emit the desugared Chain+Parallel+constant subtree. When no config, emit plain Invoke.
- **Handler runtime (TS)**: Instead of the TS runtime injecting `stepConfig` from the AST node, the pipeline constructs the handler's full input. The handler's `handle` function signature doesn't change — it still receives `{ value, stepConfig }` — but the source of the config is the pipeline, not the runtime.
- **`createHandler` API**: Unchanged from the user's perspective.

## Why do this

1. **Simpler Rust AST.** `HandlerKind` loses an `Option<Value>` field. The engine has zero awareness of handler configs — one less concept.
2. **Configs become first-class pipeline values.** They flow through the same compositional algebra as everything else. You could dynamically compute configs, branch on them, etc.
3. **The TS sugar hides the complexity.** Users never see the desugared AST. `setup({ stepConfig: { timeout: 5000 } })` works identically.

## Why not do this (yet)

1. **Requires Builtin handler kind.** The desugaring uses `identity()`, `drop()`, `constant()`, `tag()`, `merge()` — all of which are currently TS-side builtins implemented as `__builtin__` Invoke nodes that call TS handlers. For this desugaring to make sense in the Rust engine (not just serialization), we need the Builtin handler kind so Rust can execute `identity`, `drop`, `constant`, `tag`, and `merge` natively. Otherwise every configured handler invocation adds 5+ FFI round-trips.

2. **AST bloat.** A simple `setup({ stepConfig: ... })` becomes a Chain+Parallel+identity+drop+constant+tag+merge+Invoke subtree — 8 nodes instead of 1. In the flat table, that's ~12-15 entries per configured invocation. For a workflow with many configured handlers, this is significant.

3. **Debuggability.** The desugared AST is harder to inspect. A `{ stepConfigSchema: { timeout: 5000 } }` field on the Invoke node is immediately readable. A Chain+Parallel subtree requires understanding the desugaring pattern to see that it's "just" injecting a config.

4. **Partial application isn't needed.** The current model works fine — config is static data, not a pipeline value. Making it a pipeline value adds compositional power that nobody has asked for. YAGNI.

## Verdict

Theoretically clean but practically premature. The key blocker is the Builtin handler kind — without native Rust execution of data transforms, desugaring adds FFI overhead per configured invocation. Once Builtins exist (deferred feature), revisit. Until then, `step_config_schema` on `TypeScriptHandler` is a reasonable pragmatic choice.

If we want to move toward this, the implementation order is:
1. Implement Builtin handler kind (identity, constant, drop, tag, merge as native Rust handlers)
2. Change `invoke()` / callable handler to emit desugared AST when config is present
3. Remove `step_config_schema` from `TypeScriptHandler`
4. Update handler runtime to receive config from pipeline instead of AST metadata

## Multiple arguments

The same Parallel pattern handles multiple arguments. If a handler needs `(value, config, extraArg)`, the pipeline constructs:

```
Parallel([valueAction, configAction, extraAction]) → Invoke(handler)
```

The handler receives `[value, config, extra]`. There's no need for a special multi-argument mechanism — it's just tupling via Parallel.

Currently no handler needs more than `(value, stepConfig)`, so this is purely theoretical. But the pattern generalizes naturally if the need arises.

## The higher-order function question

Could the `rest` of a Chain be a "higher-order function" — an action that receives another action as input and returns a modified action? No. The pipeline only carries JSON values, not actions. There's no way to pass an action through the pipeline, partially apply it, or return a function.

The workflow algebra is intentionally first-order: actions compose statically (at build time), values flow dynamically (at runtime). Higher-order composition would require runtime action construction, which breaks the static analysis and persistence properties that make the flat table model work.

If we needed something like partial application (e.g., "given this config, return a handler bound to that config"), it would be a compile-time combinator in the TS layer, not a runtime mechanism. The TS layer already does this — `setup({ stepConfig: { timeout: 5000 } })` is effectively partial application, computed at build time and serialized as a static AST node.

## IR terminology

The system has three representation levels:

| Layer | Name | Description |
|-------|------|-------------|
| TypeScript combinators | **Surface DSL** | `pipe()`, `forEach()`, `branch()` — user-facing API |
| `Action` tree (JSON) | **AST / HIR** | Nested tree of compositional nodes. Serialized to/from JSON. |
| `FlatConfig` | **Bytecode / LIR** | Linear entry array, index-based references. Interpreter-ready. |

"Bytecode" is the most natural fit for the flat table — it's a linear, addressable, VM-ready representation analogous to JVM bytecode or WASM. The tree AST is the high-level IR. The TS combinators are the surface syntax / DSL.
