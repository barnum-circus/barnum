# Unify HandlerKind with Execution

## Motivation

`HandlerKind` and `Execution` encode the same binary split: builtin (inline) vs non-builtin (subprocess). Today they're separate enums. `HandlerKind` lives in the AST and describes handler identity. `Execution` (from the graceful shutdown refactor) lives on frames and describes runtime state.

Making `Execution` generic and defining `HandlerKind` as a type alias eliminates the redundancy and makes the relationship explicit in the type system.

## Current state

### `HandlerKind` (`crates/barnum_ast/src/lib.rs:203-208`)

```rust
pub enum HandlerKind {
    TypeScript(TypeScriptHandler),
    Builtin(BuiltinHandler),
}
```

### `Execution` (after graceful shutdown lands)

```rust
pub enum Execution {
    Inline,
    Subprocess(Pid),
}
```

### Match sites on `HandlerKind`

Every match site does one of two things:

1. **Binary split** — one branch for Builtin, one for TypeScript (which generalizes to "one for all non-builtins"):
   - `Scheduler::dispatch` — execute inline vs spawn subprocess
   - `drive_builtins` test helper — execute and complete vs collect for manual completion
   - `compile_schemas` — skip builtins, compile non-builtin schemas
   - `validate_value` — skip builtins, validate non-builtins

2. **Per-variant extraction** — needs variant-specific fields after the split:
   - Completion logging — extract module/func or builtin kind for structured logs
   - Schema error reporting — extract module/func for error messages

Category 1 uses only the binary split. Category 2 needs the payload, but always after matching the binary split first.

## Proposed change

Make `Execution` generic. `HandlerKind` becomes a type alias:

```rust
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "execution")]
pub enum Execution<TInline, TSubprocess> {
    Inline(TInline),
    Subprocess(TSubprocess),
}

/// Handler definition in the AST.
pub type HandlerKind = Execution<BuiltinHandler, TypeScriptHandler>;
```

Frame usage (from graceful shutdown):

```rust
Invoke {
    handler: HandlerId,
    execution: Execution<(), Pid>,
}
```

### Serde

`HandlerKind` currently uses `#[serde(tag = "kind")]` with variants `TypeScript` and `Builtin`. After unification, the tag becomes `"execution"` with variants `Inline` and `Subprocess`. This is a breaking wire format change. Since nobody is using this: break freely.

Alternatively, use `#[serde(rename = "...")]` on the variants to preserve the existing wire format if desired. Not worth the complexity.

### When Python arrives

```rust
pub enum SubprocessHandler {
    TypeScript(TypeScriptHandler),
    Python(PythonHandler),
}

pub type HandlerKind = Execution<BuiltinHandler, SubprocessHandler>;
```

Sites that only need the binary split still match `Inline`/`Subprocess`. Sites that need language-specific fields match further into `SubprocessHandler`.

## Impact

- `barnum_ast`: `HandlerKind` enum → type alias, `Execution` generic enum added
- `barnum_engine`: frame uses `Execution<(), Pid>`
- `barnum_event_loop`: all `HandlerKind::Builtin` matches become `Execution::Inline`, `HandlerKind::TypeScript` becomes `Execution::Subprocess`
- `barnum_builtins`: no change (receives `&BuiltinKind`, not `HandlerKind`)
- Test helpers: `HandlerKind::Builtin(...)` → `Execution::Inline(...)`, `HandlerKind::TypeScript(...)` → `Execution::Subprocess(...)`

## Open questions

1. **Naming: `Inline`/`Subprocess` vs `Builtin`/`External`?** `Inline`/`Subprocess` describes the execution model. `Builtin`/`External` describes the handler's nature. They happen to be the same thing today. `Inline`/`Subprocess` chosen because it generalizes better — a future WASM handler might be "inline" without being a "builtin."

2. **Do this as part of graceful shutdown or separately?** The graceful shutdown refactor introduces `Execution` as a non-generic enum. This refactor makes it generic and subsumes `HandlerKind`. Could land together or sequentially. Sequential is lower risk.
