# Declare Bindings — Implementation Plan

This document specifies every code change needed to implement `declare` bindings with eager, parallel evaluation. Every change has before/after code. No ambiguity — just implement top to bottom.

Design decisions are in `LET_BINDINGS.md`. This document does not discuss alternatives.

## Overview

Two new concepts:

1. **`Declare` action node**: holds a map of bindings (each an `Action`) and a body (`Action`). The scheduler evaluates all bindings in parallel with the pipeline input, stores results in an environment, then executes the body.

2. **`VarRef` builtin**: a leaf node that ignores the pipeline input and resolves to a bound value from the environment. Identified by a unique string ID (not a user-facing name).

## File-by-file changes

### 1. Rust AST — `crates/barnum_ast/src/lib.rs`

#### Add `DeclareId` newtype

```rust
// AFTER the KindDiscriminator string_key_newtype block:

string_key_newtype!(
    /// Unique identifier for a declare binding, generated at definition time
    /// in JavaScript. Used by VarRef to look up bound values in the environment.
    DeclareId
);
```

#### Add `Declare` variant to `Action` enum

```rust
// BEFORE:
pub enum Action {
    Invoke(InvokeAction),
    Chain(ChainAction),
    ForEach(ForEachAction),
    All(AllAction),
    Branch(BranchAction),
    Loop(LoopAction),
    Step(StepAction),
}

// AFTER:
pub enum Action {
    Invoke(InvokeAction),
    Chain(ChainAction),
    ForEach(ForEachAction),
    All(AllAction),
    Branch(BranchAction),
    Loop(LoopAction),
    Step(StepAction),
    Declare(DeclareAction),
}
```

#### Add `DeclareAction` struct

```rust
// After StepAction struct:

/// Scoped variable bindings. Evaluates all bindings eagerly (in parallel)
/// with the pipeline input, stores results in the environment, then
/// executes the body. VarRef nodes in the body resolve from the environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeclareAction {
    /// Map from unique binding IDs to their definition ASTs.
    /// Each binding receives the pipeline input and produces a value.
    pub bindings: HashMap<DeclareId, Action>,
    /// The body to execute with bindings in scope.
    pub body: Box<Action>,
}
```

#### Add `VarRef` variant to `BuiltinKind`

```rust
// BEFORE:
pub enum BuiltinKind {
    Constant { value: Value },
    Identity,
    Drop,
    Tag { value: Value },
    Merge,
    Flatten,
    GetField { value: Value },
    GetIndex { value: Value },
    Pick { value: Value },
}

// AFTER:
pub enum BuiltinKind {
    Constant { value: Value },
    Identity,
    Drop,
    Tag { value: Value },
    Merge,
    Flatten,
    GetField { value: Value },
    GetIndex { value: Value },
    Pick { value: Value },
    /// Resolve a declared variable from the environment.
    /// The `id` is the unique DeclareId assigned at definition time.
    VarRef { id: DeclareId },
}
```

### 2. Flat representation — `crates/barnum_ast/src/flat.rs`

#### Add `Declare` variant to `FlatAction`

```rust
// BEFORE:
pub enum FlatAction<T> {
    Invoke { handler: HandlerId },
    Chain { rest: ActionId },
    All { count: Count },
    ForEach { body: ActionId },
    Branch { count: Count },
    Loop { body: ActionId },
    Step { target: T },
}

// AFTER:
pub enum FlatAction<T> {
    Invoke { handler: HandlerId },
    Chain { rest: ActionId },
    All { count: Count },
    ForEach { body: ActionId },
    Branch { count: Count },
    Loop { body: ActionId },
    Step { target: T },
    /// Scoped bindings: evaluate `binding_count` bindings in parallel,
    /// then execute `body`. Bindings follow the Declare entry as
    /// `binding_count` pairs of (DeclareKey, child slot), same layout as Branch.
    Declare { binding_count: Count, body: ActionId },
}
```

#### Add `DeclareKey` variant to `FlatEntry`

```rust
// BEFORE:
pub enum FlatEntry<T> {
    Action(FlatAction<T>),
    ChildRef { action: ActionId },
    BranchKey { key: KindDiscriminator },
}

// AFTER:
pub enum FlatEntry<T> {
    Action(FlatAction<T>),
    ChildRef { action: ActionId },
    BranchKey { key: KindDiscriminator },
    /// Binding ID for a Declare entry. Same role as BranchKey for Branch.
    DeclareKey { id: DeclareId },
}
```

**Note:** Adding `DeclareKey` increases `FlatEntry<ActionId>` size. Verify the static assertion still holds. If it exceeds 8 bytes (likely due to `DeclareId` being a `StringKey`), we may need to adjust the assertion. `BranchKey` has the same issue — both hold interned strings. The assertion may already be > 8 bytes with `BranchKey`; check and update the `const _` assertion accordingly.

#### Update `try_map_target` for `FlatAction`

```rust
// Add Declare arm to the match:
FlatAction::Declare { binding_count, body } => FlatAction::Declare { binding_count, body },
```

#### Update `try_map_target` for `FlatEntry`

```rust
// Add DeclareKey arm to the match:
FlatEntry::DeclareKey { id } => FlatEntry::DeclareKey { id },
```

#### Add `declare_bindings` accessor on `FlatConfig`

```rust
/// Returns (id, action) pairs for a Declare node's bindings.
pub fn declare_bindings(
    &self,
    id: ActionId,
) -> impl Iterator<Item = (DeclareId, ActionId)> + '_ {
    let binding_count = match self.action(id) {
        FlatAction::Declare { binding_count, .. } => binding_count.0,
        other => panic!("expected Declare, got {other:?}"),
    };
    (0..binding_count).map(move |i| {
        let key_slot = id + 1 + 2 * i;
        let declare_id = match self.entries[key_slot.0 as usize] {
            FlatEntry::DeclareKey { id } => id,
            ref other => panic!("expected DeclareKey at {key_slot:?}, got {other:?}"),
        };
        let child_slot = key_slot + 1;
        (declare_id, self.resolve_child_slot(child_slot))
    })
}
```

#### Update `flatten_action_at`

Add the Declare case in the match, right before the `Step` arms:

```rust
Action::Declare(crate::DeclareAction { bindings, body }) => {
    let binding_count = Count(bindings.len() as u32);
    // Sort bindings by key for deterministic flattening.
    let mut bindings: Vec<_> = bindings.into_iter().collect();
    bindings.sort_by_key(|(key, _)| *key);
    // Allocate key+child slot pairs (same layout as Branch).
    self.alloc_many(Count(2 * binding_count.0));
    for (i, (declare_id, binding_action)) in bindings.into_iter().enumerate() {
        let key_slot = action_id + 1 + 2 * i as u32;
        self.entries[key_slot.0 as usize] = Some(FlatEntry::DeclareKey { id: declare_id });
        self.fill_child_slot(binding_action, key_slot + 1, workflow_root)?;
    }
    let body_id = self.flatten_action(*body, workflow_root)?;
    FlatAction::Declare { binding_count, body: body_id }
}
```

#### Update `fill_child_slot`

Declare is a multi-entry action — add it to the match for multi-entry dispatch:

```rust
// BEFORE:
Action::Chain { .. } | Action::All { .. } | Action::Branch { .. } => {

// AFTER:
Action::Chain { .. } | Action::All { .. } | Action::Branch { .. } | Action::Declare { .. } => {
```

#### Update import in `flat.rs`

Add `DeclareId` to the import from `crate`:

```rust
// BEFORE:
use crate::{
    Action, BranchAction, ChainAction, Config, HandlerKind, InvokeAction, KindDiscriminator,
    StepName, StepRef,
};

// AFTER:
use crate::{
    Action, BranchAction, ChainAction, Config, DeclareId, HandlerKind, InvokeAction,
    KindDiscriminator, StepName, StepRef,
};
```

### 3. Engine frames — `crates/barnum_engine/src/frame.rs`

#### Add `Declare` variant to `FrameKind`

```rust
// AFTER Loop variant:

/// Scoped bindings: collecting binding results before executing body.
Declare {
    /// The body action to execute after all bindings complete.
    body: ActionId,
    /// Slot per binding; `None` until the binding completes.
    /// Indexed by sorted order from flattening.
    results: Vec<Option<Value>>,
    /// The DeclareIds in sorted order, parallel to `results`.
    ids: Vec<DeclareId>,
    /// The parent environment (from enclosing Declare scopes).
    parent_env: Environment,
}
```

#### Add `Environment` type

```rust
use std::collections::HashMap;

/// Immutable variable environment for Declare scopes.
/// Maps DeclareId → cached Value. Shared across forEach iterations
/// and parallel branches (read-only, never mutated after construction).
#[derive(Debug, Clone, Default)]
pub struct Environment {
    bindings: HashMap<DeclareId, Value>,
}

impl Environment {
    /// Create an empty environment.
    pub fn new() -> Self {
        Self { bindings: HashMap::new() }
    }

    /// Look up a binding by ID. Panics if not found (indicates a bug —
    /// VarRef should only reference in-scope bindings).
    #[must_use]
    pub fn get(&self, id: &DeclareId) -> &Value {
        self.bindings.get(id).unwrap_or_else(|| {
            panic!("VarRef references unknown DeclareId: {id:?}")
        })
    }

    /// Create a new environment extending this one with additional bindings.
    pub fn extend(&self, new_bindings: impl Iterator<Item = (DeclareId, Value)>) -> Self {
        let mut bindings = self.bindings.clone();
        bindings.extend(new_bindings);
        Self { bindings }
    }
}
```

Add `DeclareId` to imports:

```rust
use barnum_ast::DeclareId;
```

### 4. Engine — `crates/barnum_engine/src/lib.rs`

#### Add environment to `WorkflowState`

The engine needs to thread the current environment through advance calls. Add an environment field or pass it through advance. The simplest approach: store a "current environment" stack that Declare frames push/pop.

However, since `advance` is recursive and environments are scoped, the cleanest approach is to pass the environment as a parameter to `advance`:

```rust
// BEFORE:
pub fn advance(
    &mut self,
    action_id: ActionId,
    value: Value,
    parent: Option<ParentRef>,
) -> Result<(), AdvanceError> {

// AFTER:
pub fn advance(
    &mut self,
    action_id: ActionId,
    value: Value,
    parent: Option<ParentRef>,
    env: &Environment,
) -> Result<(), AdvanceError> {
```

**Every existing call site of `advance` needs to pass `env`.** For the public API, add a convenience method:

```rust
/// Start execution from the workflow root with an empty environment.
/// This is the public entry point; internal recursion uses advance() directly.
pub fn start(
    &mut self,
    action_id: ActionId,
    value: Value,
) -> Result<(), AdvanceError> {
    self.advance(action_id, value, None, &Environment::new())
}
```

Update the existing `advance` body — all recursive `self.advance(...)` calls must pass `env` as the last argument. The environment is immutable within a scope, so all children receive the same `env` reference.

#### Handle `Declare` in `advance`

Add this match arm:

```rust
FlatAction::Declare { binding_count, body } => {
    if binding_count.0 == 0 {
        // No bindings — just execute the body with current env.
        self.advance(body, value, parent, env)?;
        return Ok(());
    }
    // Collect binding info to release immutable borrow.
    let bindings: Vec<(DeclareId, ActionId)> =
        self.flat_config.declare_bindings(action_id).collect();
    let frame_id = self.insert_frame(Frame {
        parent,
        kind: FrameKind::Declare {
            body,
            results: vec![None; binding_count.0 as usize],
            ids: bindings.iter().map(|(id, _)| *id).collect(),
            parent_env: env.clone(),
        },
    });
    // Evaluate all bindings in parallel with the pipeline input.
    for (i, (_, binding_action_id)) in bindings.into_iter().enumerate() {
        self.advance(
            binding_action_id,
            value.clone(),
            Some(ParentRef::IndexedChild {
                frame_id,
                child_index: i,
            }),
            env,
        )?;
    }
}
```

#### Handle `VarRef` in `advance`

VarRef is a builtin, but it needs access to the environment, which `execute_builtin` doesn't have. Handle it specially in the Invoke arm:

```rust
FlatAction::Invoke { handler } => {
    let handler_kind = self.flat_config.handler(handler);
    // Check for VarRef — resolved inline, not dispatched.
    if let HandlerKind::Builtin(barnum_ast::BuiltinHandler {
        builtin: BuiltinKind::VarRef { id },
    }) = handler_kind {
        let resolved_value = env.get(id).clone();
        // Deliver immediately — no dispatch, no task ID.
        self.deliver(parent, resolved_value)?;
        return Ok(());
    }
    let task_id = self.next_task_id();
    self.task_to_parent.insert(task_id, parent);
    self.pending_dispatches.push(Dispatch {
        task_id,
        handler_id: handler,
        value,
    });
}
```

This requires adding `BuiltinKind` to the engine's imports:

```rust
use barnum_ast::BuiltinKind;
```

#### Handle `Declare` in `deliver`

When all binding results are collected, construct the new environment and advance the body:

```rust
// In the IndexedChild match arm, extend the pattern:

// BEFORE:
FrameKind::All { results } | FrameKind::ForEach { results } => {

// AFTER — add a separate arm for Declare before the All/ForEach arm:
FrameKind::Declare { body, results, ids, parent_env } => {
    results[child_index] = Some(value);
    if results.iter().all(Option::is_some) {
        // All bindings evaluated. Build extended environment.
        let new_bindings = ids.into_iter().zip(
            results.into_iter().map(|r| r.unwrap())
        );
        let new_env = parent_env.extend(new_bindings);
        let parent = frame.parent;
        self.frames.remove(frame_id.0);
        // Execute body with extended environment.
        // Body receives the original pipeline input — but we don't have it
        // anymore. Wait — we need to store the pipeline input in the frame.
        // See note below.
    }
}
```

**Critical issue**: When all bindings complete, we need to advance the body with the **original pipeline input** (the value that entered the Declare node), not any binding's output. But that value isn't stored anywhere after the bindings are dispatched.

**Fix**: Store the pipeline input in the Declare frame.

```rust
// Updated FrameKind::Declare:
Declare {
    body: ActionId,
    results: Vec<Option<Value>>,
    ids: Vec<DeclareId>,
    parent_env: Environment,
    /// The pipeline input to the Declare node, preserved for the body.
    pipeline_input: Value,
}
```

And in `advance`, store it:

```rust
kind: FrameKind::Declare {
    body,
    results: vec![None; binding_count.0 as usize],
    ids: bindings.iter().map(|(id, _)| *id).collect(),
    parent_env: env.clone(),
    pipeline_input: value.clone(),  // <-- store for body
},
```

Complete `deliver` for Declare:

```rust
FrameKind::Declare { body, results, ids, parent_env, pipeline_input } => {
    results[child_index] = Some(value);
    if results.iter().all(Option::is_some) {
        let new_bindings = ids.into_iter().zip(
            results.into_iter().map(|r| r.unwrap())
        );
        let new_env = parent_env.extend(new_bindings);
        let parent = frame.parent;
        self.frames.remove(frame_id.0);
        self.advance(body, pipeline_input, parent, &new_env)?;
        Ok(None)
    } else {
        Ok(None)
    }
}
```

#### Update `deliver` to pass environment

The `deliver` method currently calls `self.advance(...)` for Chain trampolines and Loop re-entry. These calls also need the environment. Since `deliver` is called from `complete` (which doesn't know the environment), we need to store the environment in frames that call advance during delivery.

**Chain and Loop frames need an `env` field:**

```rust
// Updated FrameKind:
pub enum FrameKind {
    Chain {
        rest: ActionId,
        env: Environment,
    },
    All {
        results: Vec<Option<Value>>,
    },
    ForEach {
        results: Vec<Option<Value>>,
    },
    Loop {
        body: ActionId,
        env: Environment,
    },
    Declare {
        body: ActionId,
        results: Vec<Option<Value>>,
        ids: Vec<DeclareId>,
        parent_env: Environment,
        pipeline_input: Value,
    },
}
```

Update frame construction in `advance` for Chain:

```rust
FlatAction::Chain { rest } => {
    let first = self.flat_config.chain_first(action_id);
    let frame_id = self.insert_frame(Frame {
        parent,
        kind: FrameKind::Chain { rest, env: env.clone() },
    });
    self.advance(first, value, Some(ParentRef::SingleChild { frame_id }), env)?;
}
```

Update frame construction in `advance` for Loop:

```rust
FlatAction::Loop { body } => {
    let frame_id = self.insert_frame(Frame {
        parent,
        kind: FrameKind::Loop { body, env: env.clone() },
    });
    self.advance(body, value, Some(ParentRef::SingleChild { frame_id }), env)?;
}
```

Update `deliver` for Chain:

```rust
FrameKind::Chain { rest, env } => {
    self.advance(rest, value, frame.parent, &env)?;
    Ok(None)
}
```

Update `deliver` for Loop:

```rust
FrameKind::Loop { body, env } => match value["kind"].as_str() {
    Some("Continue") => {
        let frame_id = self.insert_frame(Frame {
            parent: frame.parent,
            kind: FrameKind::Loop { body, env: env.clone() },
        });
        self.advance(
            body,
            value["value"].clone(),
            Some(ParentRef::SingleChild { frame_id }),
            &env,
        )?;
        Ok(None)
    }
    Some("Break") => self.deliver(frame.parent, value["value"].clone()),
    _ => Err(CompleteError::InvalidLoopResult { value }),
},
```

All and ForEach don't call advance during delivery — they just collect results. No changes needed for those.

### 5. Builtins — `crates/barnum_builtins/src/lib.rs`

Add VarRef to the match in `execute_builtin`. Since VarRef is handled specially by the engine (it resolves from the environment, not from the input), the builtin execution should never be reached:

```rust
BuiltinKind::VarRef { .. } => {
    panic!("VarRef should be resolved by the engine, not executed as a builtin")
}
```

### 6. TypeScript AST — `libs/barnum/src/ast.ts`

#### Add `DeclareAction` to the `Action` union

```typescript
// BEFORE:
export type Action =
  | InvokeAction
  | ChainAction
  | ForEachAction
  | AllAction
  | BranchAction
  | LoopAction
  | StepAction;

// AFTER:
export type Action =
  | InvokeAction
  | ChainAction
  | ForEachAction
  | AllAction
  | BranchAction
  | LoopAction
  | StepAction
  | DeclareAction;
```

#### Add `DeclareAction` interface

```typescript
export interface DeclareAction {
  kind: "Declare";
  bindings: Record<string, Action>;
  body: Action;
}
```

#### Add `VarRef` to `BuiltinKind`

```typescript
// BEFORE:
export type BuiltinKind =
  | { kind: "Constant"; value: unknown }
  | { kind: "Identity" }
  | { kind: "Drop" }
  | { kind: "Tag"; value: string }
  | { kind: "Merge" }
  | { kind: "Flatten" }
  | { kind: "GetField"; value: string }
  | { kind: "GetIndex"; value: number }
  | { kind: "Pick"; value: string[] };

// AFTER:
export type BuiltinKind =
  | { kind: "Constant"; value: unknown }
  | { kind: "Identity" }
  | { kind: "Drop" }
  | { kind: "Tag"; value: string }
  | { kind: "Merge" }
  | { kind: "Flatten" }
  | { kind: "GetField"; value: string }
  | { kind: "GetIndex"; value: number }
  | { kind: "Pick"; value: string[] }
  | { kind: "VarRef"; id: string };
```

#### Add the `declare` combinator function

Add to `ast.ts`, after the `loop` function:

```typescript
// ---------------------------------------------------------------------------
// Declare — scoped variable bindings
// ---------------------------------------------------------------------------

/**
 * Extract the output type from a TypedAction, used for binding type inference.
 * Re-exported here for use in declare's type signature.
 */
type DeclareBindings<TIn, TBindings extends Record<string, Pipeable<TIn, unknown>>> = {
  [K in keyof TBindings]: TypedAction<never, ExtractOutput<TBindings[K]>>;
};

/** Counter for generating unique DeclareIds. */
let nextDeclareId = 0;

/**
 * Scoped variable bindings. Evaluates all bindings eagerly (in parallel)
 * with the pipeline input, then executes the body with the bindings in scope.
 *
 * Each binding is an AST that receives the pipeline input and produces a value.
 * The body callback receives typed AST references (VarRef nodes) for each
 * binding. These references are `TypedAction<never, T>` — they take no
 * pipeline input (the value is already captured in the environment).
 *
 * The body also receives the pipeline input, so bindings are supplementary,
 * not a replacement for the pipeline value.
 *
 * @example
 * ```ts
 * declare({
 *   branch: pipe(getField("description"), deriveBranch),
 * }, ({ branch }) =>
 *   pipe(
 *     implement,
 *     branch.then(createPR),
 *   ),
 * )
 * ```
 */
export function declare<
  TIn,
  TBindings extends Record<string, Pipeable<TIn, unknown>>,
  TOut,
  TRefs extends string = never,
>(
  bindings: TBindings,
  body: (
    vars: DeclareBindings<TIn, TBindings>,
  ) => Pipeable<TIn, TOut, TRefs>,
): TypedAction<TIn, TOut, TRefs> {
  // Generate unique IDs for each binding and create VarRef AST nodes.
  const bindingEntries: Record<string, Action> = {};
  const vars: Record<string, TypedAction> = {};

  for (const key of Object.keys(bindings)) {
    const id = `__declare_${nextDeclareId++}`;
    bindingEntries[id] = bindings[key] as Action;
    vars[key] = typedAction<never, unknown>({
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "VarRef", id } },
    });
  }

  // Run the body callback at definition time to produce the body AST.
  const bodyAction = body(vars as DeclareBindings<TIn, TBindings>);

  return typedAction<TIn, TOut, TRefs>({
    kind: "Declare",
    bindings: bindingEntries,
    body: bodyAction as Action,
  });
}
```

#### Export `declare`

Add to the combinators section:

```typescript
// In the combinators export area, no separate file needed since declare
// is defined inline in ast.ts. If you prefer a separate file, create
// libs/barnum/src/declare.ts and export from ast.ts.
```

Since `declare` is defined in `ast.ts` itself (like `forEach`, `branch`, `loop`), it's automatically available via `ast.ts` imports.

### 7. Generated schemas

Run `cargo run -p barnum_cli --bin build_schemas` after the Rust changes. This regenerates:
- `libs/barnum/barnum-config-schema.json`
- `libs/barnum/barnum-config-schema.zod.ts`
- `libs/barnum/barnum-cli-schema.zod.ts`

No manual edits to these files.

### 8. Event loop — `crates/barnum_event_loop/src/lib.rs`

The event loop calls `engine.advance(root, input, None)`. Update to use the new `start` method:

```rust
// BEFORE:
engine.advance(root, input, None)?;

// AFTER:
engine.start(root, input)?;
```

No other changes — the event loop dispatches handlers and calls `complete`, which handles Declare frames internally.

## Tests

### Rust unit tests — `crates/barnum_ast/src/flat.rs`

Add test helpers and tests:

```rust
/// Helper: create a Declare action.
fn declare(bindings: Vec<(&str, Action)>, body: Action) -> Action {
    use crate::DeclareId;
    use intern::string_key::Intern;
    Action::Declare(crate::DeclareAction {
        bindings: bindings
            .into_iter()
            .map(|(id, action)| (DeclareId::from(id.intern()), action))
            .collect(),
        body: Box::new(body),
    })
}

#[test]
#[allow(clippy::unwrap_used)]
fn flatten_declare_no_bindings() {
    let flat = flatten(config(declare(vec![], invoke("./handler.ts", "run")))).unwrap();
    assert_eq!(
        flat.action(ActionId(0)),
        FlatAction::Declare { binding_count: Count(0), body: ActionId(1) }
    );
}

#[test]
#[allow(clippy::unwrap_used)]
fn flatten_declare_with_bindings() {
    let flat = flatten(config(declare(
        vec![
            ("x", invoke("./x.ts", "compute_x")),
            ("y", invoke("./y.ts", "compute_y")),
        ],
        invoke("./body.ts", "run"),
    ))).unwrap();

    assert_eq!(
        flat.action(ActionId(0)),
        FlatAction::Declare { binding_count: Count(2), body: flat.action(ActionId(0)).body() }
    );
    // Verify bindings are accessible
    let bindings: Vec<_> = flat.declare_bindings(ActionId(0)).collect();
    assert_eq!(bindings.len(), 2);
}

#[test]
#[allow(clippy::unwrap_used)]
fn flatten_declare_is_multi_entry() {
    // Declare inside a Chain child slot should produce ChildRef.
    let action = pipe(vec![
        declare(vec![("x", invoke("./x.ts", "x"))], invoke("./body.ts", "run")),
        invoke("./after.ts", "run"),
    ]);
    let flat = flatten(config(action)).unwrap();
    // Chain at 0 should have ChildRef at slot 1 pointing to Declare elsewhere.
    assert_eq!(flat.action(ActionId(0)), FlatAction::Chain { rest: _ });
}
```

### Rust unit tests — `crates/barnum_engine/src/lib.rs`

Add test helpers:

```rust
use barnum_ast::{DeclareAction, DeclareId, BuiltinHandler, BuiltinKind};

fn declare_action(bindings: Vec<(&str, Action)>, body: Action) -> Action {
    Action::Declare(DeclareAction {
        bindings: bindings
            .into_iter()
            .map(|(id, action)| (DeclareId::from(id.intern()), action))
            .collect(),
        body: Box::new(body),
    })
}

fn varref(id: &str) -> Action {
    Action::Invoke(InvokeAction {
        handler: HandlerKind::Builtin(BuiltinHandler {
            builtin: BuiltinKind::VarRef {
                id: DeclareId::from(id.intern()),
            },
        }),
    })
}
```

Add tests:

```rust
/// Declare with one binding: binding dispatched, then body dispatched after completion.
#[test]
#[allow(clippy::unwrap_used)]
fn declare_dispatches_bindings() {
    let mut engine = engine_from(declare_action(
        vec![("x", invoke("./compute_x.ts", "run"))],
        invoke("./body.ts", "run"),
    ));
    let root = engine.workflow_root();
    engine.start(root, json!({"input": 1})).unwrap();

    let dispatches = engine.take_pending_dispatches();
    // Only the binding is dispatched initially (body waits).
    assert_eq!(dispatches.len(), 1);
    assert_eq!(
        engine.handler(dispatches[0].handler_id),
        &ts_handler("./compute_x.ts", "run"),
    );
    assert_eq!(dispatches[0].value, json!({"input": 1}));
}

/// Declare: completing bindings triggers body with original pipeline input.
#[test]
#[allow(clippy::unwrap_used)]
fn declare_body_receives_pipeline_input() {
    let mut engine = engine_from(declare_action(
        vec![("x", invoke("./compute_x.ts", "run"))],
        invoke("./body.ts", "run"),
    ));
    let root = engine.workflow_root();
    engine.start(root, json!({"input": 1})).unwrap();

    let d = engine.take_pending_dispatches();
    // Complete the binding.
    engine.complete(d[0].task_id, json!("x_value")).unwrap();

    // Body should be dispatched with the original pipeline input.
    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 1);
    assert_eq!(d[0].value, json!({"input": 1}));  // NOT "x_value"
    assert_eq!(
        engine.handler(d[0].handler_id),
        &ts_handler("./body.ts", "run"),
    );
}

/// VarRef resolves from environment without dispatching.
#[test]
#[allow(clippy::unwrap_used)]
fn declare_varref_resolves() {
    // declare({ x: compute_x }, varref("x"))
    // Body is a VarRef — should resolve to the binding value immediately.
    let mut engine = engine_from(declare_action(
        vec![("x", invoke("./compute_x.ts", "run"))],
        varref("x"),
    ));
    let root = engine.workflow_root();
    engine.start(root, json!({"input": 1})).unwrap();

    // Binding dispatched.
    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 1);

    // Complete binding → VarRef resolves → workflow done.
    let result = engine.complete(d[0].task_id, json!("x_result")).unwrap();
    assert_eq!(result, Some(json!("x_result")));
}

/// Parallel bindings: multiple bindings dispatched simultaneously.
#[test]
#[allow(clippy::unwrap_used)]
fn declare_parallel_bindings() {
    let mut engine = engine_from(declare_action(
        vec![
            ("x", invoke("./x.ts", "run")),
            ("y", invoke("./y.ts", "run")),
        ],
        invoke("./body.ts", "run"),
    ));
    let root = engine.workflow_root();
    engine.start(root, json!("input")).unwrap();

    let dispatches = engine.take_pending_dispatches();
    assert_eq!(dispatches.len(), 2);  // Both bindings dispatched in parallel.
}

/// Nested declare: inner VarRef resolves from inner scope, outer from outer.
#[test]
#[allow(clippy::unwrap_used)]
fn declare_nested_scopes() {
    // declare({ x: compute_x },
    //   declare({ y: compute_y },
    //     all(varref("x"), varref("y"))
    //   )
    // )
    let mut engine = engine_from(declare_action(
        vec![("x", invoke("./x.ts", "run"))],
        declare_action(
            vec![("y", invoke("./y.ts", "run"))],
            all(vec![varref("x"), varref("y")]),
        ),
    ));
    let root = engine.workflow_root();
    engine.start(root, json!("input")).unwrap();

    // Outer binding dispatched.
    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 1);

    // Complete outer binding → inner Declare starts → inner binding dispatched.
    engine.complete(d[0].task_id, json!("x_val")).unwrap();
    let d = engine.take_pending_dispatches();
    assert_eq!(d.len(), 1);

    // Complete inner binding → body executes → both VarRefs resolve.
    let result = engine.complete(d[0].task_id, json!("y_val")).unwrap();
    assert_eq!(result, Some(json!(["x_val", "y_val"])));
}

/// Declare with no bindings: body executes immediately.
#[test]
#[allow(clippy::unwrap_used)]
fn declare_empty_bindings() {
    let mut engine = engine_from(declare_action(
        vec![],
        invoke("./body.ts", "run"),
    ));
    let root = engine.workflow_root();
    engine.start(root, json!("input")).unwrap();

    let dispatches = engine.take_pending_dispatches();
    assert_eq!(dispatches.len(), 1);
    assert_eq!(dispatches[0].value, json!("input"));
}
```

### Snapshot tests — `crates/barnum_engine/tests/`

Add test case files:

**`tests/advance/declare_single_binding.json`:**
```json
{
  "config": {
    "workflow": {
      "kind": "Declare",
      "bindings": {
        "__declare_0": {
          "kind": "Invoke",
          "handler": { "kind": "TypeScript", "module": "./compute.ts", "func": "run" }
        }
      },
      "body": {
        "kind": "Invoke",
        "handler": { "kind": "Builtin", "builtin": { "kind": "VarRef", "id": "__declare_0" } }
      }
    }
  },
  "input": { "x": 1 }
}
```

**`tests/advance/declare_multiple_bindings.json`:**
```json
{
  "config": {
    "workflow": {
      "kind": "Declare",
      "bindings": {
        "__declare_0": {
          "kind": "Invoke",
          "handler": { "kind": "TypeScript", "module": "./x.ts", "func": "run" }
        },
        "__declare_1": {
          "kind": "Invoke",
          "handler": { "kind": "TypeScript", "module": "./y.ts", "func": "run" }
        }
      },
      "body": {
        "kind": "Invoke",
        "handler": { "kind": "TypeScript", "module": "./body.ts", "func": "run" }
      }
    }
  },
  "input": "shared_input"
}
```

**`tests/completion/declare_varref.json`:**
```json
{
  "config": {
    "workflow": {
      "kind": "Declare",
      "bindings": {
        "__declare_0": {
          "kind": "Invoke",
          "handler": { "kind": "TypeScript", "module": "./compute.ts", "func": "run" }
        }
      },
      "body": {
        "kind": "Invoke",
        "handler": { "kind": "Builtin", "builtin": { "kind": "VarRef", "id": "__declare_0" } }
      }
    }
  },
  "input": { "x": 1 },
  "completions": [
    { "task_id": 0, "value": "computed_result" }
  ]
}
```

### TypeScript round-trip test — `libs/barnum/tests/round-trip.test.ts`

```typescript
it("Declare", () => {
  const cfg = workflowBuilder().workflow(() =>
    declare({
      x: constant(42),
    }, ({ x }) =>
      x,
    ),
  );
  expect(roundTrip(cfg)).toEqual(cfg);
});

it("Declare with body pipeline", () => {
  const cfg = workflowBuilder().workflow(() =>
    declare({
      branch: pipe(constant("main"), getField<{ name: string }, "name">("name")),
    }, ({ branch }) =>
      pipe(
        constant({ artifact: "test" }),
        verify,
      ),
    ),
  );
  expect(roundTrip(cfg)).toEqual(cfg);
});
```

Add `declare` and `verify` to the imports at the top of the file.

### TypeScript type tests — `libs/barnum/tests/types.test.ts`

```typescript
// ---------------------------------------------------------------------------
// Declare binding types
// ---------------------------------------------------------------------------

describe("declare binding types", () => {
  it("basic: binding type flows to VarRef output", () => {
    const action = declare({
      x: constant(42),
    }, ({ x }) => x);
    assertExact<IsExact<ExtractInput<typeof action>, never>>();
    assertExact<IsExact<ExtractOutput<typeof action>, number>>();
  });

  it("VarRef is TypedAction<never, T>", () => {
    declare({
      x: constant({ name: "test" }),
    }, ({ x }) => {
      assertExact<IsExact<ExtractInput<typeof x>, never>>();
      assertExact<IsExact<ExtractOutput<typeof x>, { name: string }>>();
      return x;
    });
  });

  it("body receives pipeline input type", () => {
    const action = declare({
      x: pipe(getField<{ project: string }, "project">("project"), setup),
    }, ({ x }) =>
      // Body's input is { project: string } (same as declare's input).
      // x is TypedAction<never, { initialized: boolean; project: string }>.
      pipe(
        setup,  // expects { project: string }
        build,
      ),
    );
    assertExact<IsExact<ExtractInput<typeof action>, { project: string }>>();
    assertExact<IsExact<ExtractOutput<typeof action>, { artifact: string }>>();
  });

  it("multiple bindings: each has correct type", () => {
    declare({
      v: verify,
      d: deploy,
    }, ({ v, d }) => {
      assertExact<IsExact<ExtractOutput<typeof v>, { verified: boolean }>>();
      assertExact<IsExact<ExtractOutput<typeof d>, { deployed: boolean }>>();
      return constant(true);
    });
  });

  it("rejects accessing a variable not in bindings", () => {
    declare({
      x: constant(42),
    }, (vars) => {
      // @ts-expect-error — 'y' does not exist in bindings
      vars.y;
      return constant(true);
    });
  });

  it("rejects binding with wrong input type", () => {
    // declare's input is never (from constant). Binding must accept never.
    // verify expects { artifact: string }, not never.
    // @ts-expect-error — verify expects { artifact: string } but declare input is never
    declare({
      x: verify,
    }, ({ x }) => x);
  });

  it("nested declare: inner body sees both scopes' variables", () => {
    const action = declare({
      outer: constant(1),
    }, ({ outer }) =>
      declare({
        inner: constant("hello"),
      }, ({ inner }) =>
        // Both outer and inner are accessible.
        all(outer, inner),
      ),
    );
    assertExact<IsExact<ExtractOutput<typeof action>, [number, string]>>();
  });

  it("VarRef usable in pipe with .then()", () => {
    const action = declare({
      data: constant({ name: "test", value: 42 }),
    }, ({ data }) =>
      data.get("name"),
    );
    assertExact<IsExact<ExtractOutput<typeof action>, string>>();
  });
});
```

Add `declare` to the imports at the top of the file.

### TypeScript pattern tests — `libs/barnum/tests/patterns.test.ts`

```typescript
it("declare: bindings produce Declare AST node with VarRef body", () => {
  const action = declare({
    x: constant(42),
  }, ({ x }) => x);
  expect(action.kind).toBe("Declare");
  const declareNode = action as DeclareAction;
  expect(Object.keys(declareNode.bindings).length).toBe(1);
  // Body should be an Invoke with VarRef builtin.
  expect(declareNode.body.kind).toBe("Invoke");
});
```

Add `declare`, `DeclareAction` to imports.

### Demo — update an existing demo or add a declare example

Update `demos/convert-folder-to-ts/run.ts` to demonstrate declare replacing a tap/augment pattern. Or add a new focused example. The exact demo change depends on which pattern benefits most — the `identify-and-address-refactors` demo with its `withResource` and `tap` patterns is the best candidate, but requires more exploration of that specific demo's code.

At minimum, add a comment in the demo showing the before/after.

## Deferred: handler definition deduplication in the JS AST

### The problem

`declare` makes handler duplication worse. A VarRef used 5 times in the body produces 5 copies of the same AST subtree in the serialized JSON:

```json
{
  "kind": "Invoke",
  "handler": { "kind": "Builtin", "builtin": { "kind": "VarRef", "id": "__declare_0" } }
}
```

This is small for VarRef builtins, but the general problem predates `declare`. Any handler used multiple times — `identity()` in every `augment`, a user handler referenced from multiple pipeline positions — is fully inlined at every use site. The serialized config duplicates the entire handler definition (module path, function name, or builtin kind) at every occurrence.

### The solution: handler IDs in the JS AST

The Rust flat representation already solves this: `HandlerId` indexes into a deduplicated handler pool, and `FlatAction::Invoke { handler: HandlerId }` carries just the index. The JS AST should do the same.

Add a top-level `handlers` map to the `Config` type, and reference handlers by ID in the AST:

```ts
// BEFORE:
interface Config {
  workflow: Action;
  steps?: Record<string, Action>;
}

// InvokeAction embeds the full handler definition:
interface InvokeAction {
  kind: "Invoke";
  handler: HandlerKind;  // full definition inlined at every use site
}

// AFTER:
interface Config {
  workflow: Action;
  steps?: Record<string, Action>;
  handlers: Record<string, HandlerKind>;  // deduplicated pool
}

// InvokeAction references by ID:
interface InvokeAction {
  kind: "Invoke";
  handler: string;  // key into config.handlers
}
```

At definition time, `typedAction()` (or a config-level serialization pass) assigns each unique handler an ID and deduplicates. Identical handlers (same module/func, or same builtin kind + params) share the same ID.

### Why this matters for declare

Without deduplication, a `declare` block with one binding referenced N times produces N copies of the VarRef handler. With a handler pool, it produces one entry in the pool and N one-field references. The savings compound with user handlers that are large (long module paths, complex builtin parameters).

### Implementation sketch

This is a **serialization-time transform**, not a definition-time change. The in-memory TypedAction objects can continue to embed full handler definitions. The deduplication happens in `Config.toJSON()` / `RunnableConfig.toJSON()`:

```ts
class RunnableConfig {
  toJSON(): Config {
    const handlers: Record<string, HandlerKind> = {};
    const handlerToId = new Map<string, string>();
    let nextHandlerId = 0;

    function intern(handler: HandlerKind): string {
      const key = JSON.stringify(handler);  // structural equality
      let id = handlerToId.get(key);
      if (id === undefined) {
        id = `__handler_${nextHandlerId++}`;
        handlerToId.set(key, id);
        handlers[id] = handler;
      }
      return id;
    }

    // Walk the AST, replacing every InvokeAction.handler with its interned ID.
    const workflow = rewriteHandlers(this.workflow, intern);
    const steps = this.steps
      ? Object.fromEntries(
          Object.entries(this.steps).map(([k, v]) => [k, rewriteHandlers(v, intern)])
        )
      : undefined;

    return { workflow, steps, handlers };
  }
}
```

The Rust deserializer would need to resolve handler IDs back to `HandlerKind` during deserialization (or the flat representation's existing interning handles it — the flattener already deduplicates handlers by value).

### Not blocking declare

This is a config-wide optimization that predates declare and benefits all handlers. It should be a separate PR. Declare works fine without it — the duplication is a serialization size issue, not a correctness issue.

## Deferred: explicit type annotations for named steps

### The problem

Named steps (`registerSteps`) have their input/output types fully inferred from the action assigned to them. When you write:

```ts
.registerSteps({ Deploy: deploy })
```

The type of `steps.Deploy` is inferred as `TypedAction<{ verified: boolean }, { deployed: boolean }>` from `deploy`'s type. When using the callback form with `stepRef`, mutual recursion makes things even more implicit — `stepRef("Fix")` returns `TypedAction<any, any, "Fix">`, and the step's actual types are only resolved by TypeScript's constraint solver working across the entire batch.

This is convenient but arguably too implicit. In a large config with many steps, there's no declaration of what a step's contract is — you have to trace through the action's combinator chain to determine what goes in and comes out. If someone changes a handler deep in a step's pipeline, the step's type changes silently, potentially breaking callers in a way that's hard to trace.

### What explicit types would look like

Require type parameters on `registerSteps`:

```ts
// Option A: type parameters on the step definitions
.registerSteps({
  Deploy: step<{ verified: boolean }, { deployed: boolean }>(deploy),
  HealthCheck: step<{ deployed: boolean }, { stable: true }>(loop(healthCheck)),
})

// Option B: type parameters on registerSteps itself
.registerSteps<{
  Deploy: TypedAction<{ verified: boolean }, { deployed: boolean }>,
  HealthCheck: TypedAction<{ deployed: boolean }, { stable: true }>,
}>({
  Deploy: deploy,
  HealthCheck: loop(healthCheck),
})
```

Option A introduces a `step()` wrapper that asserts the action's type matches the declared signature. Option B puts the type map on `registerSteps` and validates each action against its declared type.

### What this buys you

1. **Readable contracts.** You can see a step's input/output types without tracing through its combinator pipeline.
2. **Stable interfaces.** Changing a step's internals that accidentally changes its type produces an error at the step definition, not at every call site.
3. **Documentation.** The type annotation IS the documentation — it states the step's contract explicitly.

### What this costs

1. **Verbosity.** Every step needs a type annotation. For simple steps (`Deploy: deploy`), the annotation is redundant.
2. **Duplication.** The handler already declares its input/output types via its Zod validator. The step annotation repeats this.
3. **Friction.** When iterating on a step's implementation, you have to update the type annotation alongside the action. Inference handles this automatically.

### Recommendation

Not blocking declare. Worth exploring as a separate ergonomics improvement. The strongest argument for it: `stepRef` returns `TypedAction<any, any>`, which means steps involved in mutual recursion have no type safety at their boundaries. Explicit annotations would close this hole — the declared types would be used for `stepRef` resolution instead of `any`.

## Implementation order

1. **Rust AST** (`barnum_ast/src/lib.rs`): Add `DeclareId`, `DeclareAction`, `VarRef`. Pure types, no logic.
2. **Rust flat** (`barnum_ast/src/flat.rs`): Add `FlatAction::Declare`, `FlatEntry::DeclareKey`, flatten logic, accessor. Run `cargo test -p barnum_ast`.
3. **Rust frames** (`barnum_engine/src/frame.rs`): Add `Environment`, update `FrameKind` with `Declare`, add `env` to `Chain` and `Loop`.
4. **Rust engine** (`barnum_engine/src/lib.rs`): Update `advance` signature, handle Declare and VarRef, update deliver. Run `cargo test -p barnum_engine`.
5. **Rust builtins** (`barnum_builtins/src/lib.rs`): Add VarRef panic arm.
6. **Rust event loop**: Update `advance` call to `start`.
7. **Regenerate schemas**: `cargo run -p barnum_cli --bin build_schemas`.
8. **TypeScript AST** (`ast.ts`): Add `DeclareAction`, `VarRef`, `declare()` function.
9. **TypeScript tests**: Add round-trip, type, and pattern tests. Run `pnpm test`.
10. **Snapshot tests**: Add JSON test cases, run `cargo test` with `INSTA_UPDATE=1` to generate snapshots.
