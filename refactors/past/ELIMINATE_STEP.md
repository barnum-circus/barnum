# Eliminate Step

## Motivation

Steps exist to give pipelines names for reuse and recursion. This requires: `StepAction`/`StepRef` in both ASTs, `Config.steps`, `ConfigBuilder.registerSteps()` (two overloads), `stepRef()`, `ValidateStepRefs<R>`, flattening passes 1 and 2, `FlatAction::Step`, `StepName`, `StepTarget`, and associated error variants.

Naming is already solved — pipelines are values (`const x = pipe(...)`). Non-recursive function calls are just inlining: `pipe(setup, myPipeline, deploy)`. No registration, no indirection. The only thing steps provide beyond raw values is recursion (self-reference and mutual reference). That can be expressed with resumptive handlers.

## Core mechanism

`bind` uses a ResumeHandle where the handler returns a captured value. `defineRecursiveFunctions` uses a ResumeHandle where the handler *is* the function — the function body is embedded directly in the handler DAG. The function doesn't run until called (Perform fires), and the caller's pipeline is preserved across the call (resume semantics). Recursive calls fire Perform recursively within handler execution, forming a call stack of ResumePerformFrames.

## API

One type parameter — an array of `[In, Out]` tuples, one per function:

```ts
// Define the functions. Returns a curried combinator.
const withFns = defineRecursiveFunctions<[
  [ProcessIn, ProcessOut],
  [TransformIn, TransformOut],
]>(
  (fnA, fnB) => [
    // fnA: TypedAction<ProcessIn, ProcessOut>
    // fnB: TypedAction<TransformIn, TransformOut>
    pipe(process, fnB),    // A's body — can call B
    pipe(transform, fnA),  // B's body — can call A
  ]
);

// Apply to a body. Returns a TypedAction.
withFns((fnA, fnB) => pipe(setup, fnA, deploy))
```

The type parameter `TFunctions extends [unknown, unknown][]` is a single array. TypeScript maps over it to produce call tokens — `TFunctions[0]` is `[ProcessIn, ProcessOut]`, so `fnA` is `TypedAction<ProcessIn, ProcessOut>`. TypeScript can't infer these from the circular definition, so they're explicit.

The call tokens are the same values in both callbacks. They're `Chain(Tag("Call0"), ResumePerform(resumeHandlerId))` — tagged ResumePerforms. The first callback uses them for recursion inside function bodies. The second uses them for initial calls in the workflow body. Both execute inside the ResumeHandle's scope.

`self` (`Step(Root)`) is gone — the scope handler is the restart mechanism. It's just a value.

## Desugaring

`withFns((fnA, fnB) => body)` produces:

```
Chain(
  All(Identity, Constant(null)),            // [value, null] — state is unused
  ResumeHandle(resumeHandlerId,
    body: Chain(GetIndex(0), body),     // extract value, run workflow body
    handler: All(                           // return [result, null]
      Chain(
        GetIndex(0),                    // payload from [payload, state]
        Branch({
          Call0: Chain(GetField("value"), bodyA),
          Call1: Chain(GetField("value"), bodyB),
        })
      ),
      Constant(null)                        // state passthrough (unused)
    )
  )
)
```

Outer `All(Identity, Constant(null))` creates the `[value, null]` tuple that the ResumeHandle expects — same pattern as `bind`'s `All(...bindings, Identity)`. State is null (unused). Body extracts the original value with `GetIndex(0)`. Handler dispatches to function bodies by tag, returns `[result, null]` — engine delivers `result` to the perform site and writes `null` to state.

### Recursive calls

When `bodyA` calls `fnB` mid-pipeline: ResumePerform fires, engine walks ancestors and finds the enclosing ResumeHandle, creates a ResumePerformFrame, handler runs (dispatches to bodyB). `bodyA`'s pipeline is preserved — it resumes when bodyB completes. The call stack is a chain of ResumePerformFrames.

Tail recursion accumulates O(n) frames. `loop` (RestartHandle) is O(1) for tail recursion. `loop` stays for iteration; `defineRecursiveFunction` is for general recursion.

## Phases

### Phase 1: Add `defineRecursiveFunctions`

Purely additive. Implement in `libs/barnum/src/`. Desugars to ResumeHandle/ResumePerform. Tests for self-recursion, mutual recursion, non-tail calls.

### Phase 2: Migrate consumers

Non-recursive steps become `const` declarations (just inline the pipeline). `self`/`stepRef` mutual recursion migrates to `defineRecursiveFunction(s)`.

### Phase 3: Remove Step

#### Rust AST (`crates/barnum_ast/src/lib.rs`)

Delete `StepName` newtype (`lib.rs:23-26`):

```rust
// DELETE
string_key_newtype!(
    /// Named step identifier, referenced by [`StepAction`] and [`Config::steps`].
    StepName
);
```

Delete `Action::Step` variant (`lib.rs:79-80`):

```rust
// DELETE from Action enum
    /// Named step reference for mutual recursion and DAG topologies.
    Step(StepAction),
```

Delete `StepAction` struct (`lib.rs:133-138`):

```rust
// DELETE
pub struct StepAction {
    pub step: StepRef,
}
```

Delete `StepRef` enum (`lib.rs:165-176`):

```rust
// DELETE
pub enum StepRef {
    Named { name: StepName },
    Root,
}
```

Simplify `Config` — remove `steps` field (`lib.rs:266-273`):

```rust
// BEFORE
pub struct Config {
    pub workflow: Action,
    #[serde(default, skip_serializing_if = "HashMap::is_empty")]
    pub steps: HashMap<StepName, Action>,
}

// AFTER
pub struct Config {
    pub workflow: Action,
}
```

Remove `use std::collections::HashMap` if no longer needed.

#### Rust flat config (`crates/barnum_ast/src/flat.rs`)

Delete `FlatAction::Step` variant (`flat.rs:117-121`):

```rust
// DELETE from FlatAction enum
    Step {
        target: T,
    },
```

Delete `StepTarget` enum (`flat.rs:217-224`):

```rust
// DELETE
pub enum StepTarget {
    Named(StepName),
    Resolved(ActionId),
}
```

Delete `FlattenError::StepRootInStepBody` and `FlattenError::UnknownStep` (`flat.rs:229-237`):

```rust
// DELETE
    #[error("Step(Root) is only valid in the workflow tree, not in step bodies")]
    StepRootInStepBody,
    #[error("unknown step: {name}")]
    UnknownStep { name: StepName },
```

Delete `FlatAction::try_map_target` and `FlatEntry::try_map_target` (`flat.rs:139-211`). These exist solely to map `StepTarget` during resolution. Without Step, `FlatAction` is no longer generic over `T` — it uses `ActionId` directly. `FlatEntry` likewise becomes `FlatEntry` (no type parameter).

```rust
// BEFORE
pub enum FlatAction<T> { ... Step { target: T }, ... }
pub enum FlatEntry<T> { Action(FlatAction<T>), ... }

// AFTER
pub enum FlatAction { ... }  // no Step, no type parameter
pub enum FlatEntry { Action(FlatAction), ... }
```

Delete Step arms in `flatten_action_at` (`flat.rs:470-483`):

```rust
// DELETE
            Action::Step(crate::StepAction {
                step: StepRef::Named { name },
            }) => FlatAction::Step {
                target: StepTarget::Named(name),
            },

            Action::Step(crate::StepAction {
                step: StepRef::Root,
            }) => {
                let root = workflow_root.ok_or(FlattenError::StepRootInStepBody)?;
                FlatAction::Step {
                    target: StepTarget::Resolved(root),
                }
            }
```

Simplify `resolve` method (`flat.rs:542-571`). Without step targets to resolve, this method is unnecessary. The `UnresolvedFlatConfig` becomes the `FlatConfig` directly (no resolution pass). The `workflow_root` and `step_roots` parameters go away.

Simplify `flatten` function (`flat.rs:591-609`):

```rust
// BEFORE
pub fn flatten(config: Config) -> Result<FlatConfig, FlattenError> {
    let mut unresolved_flat_config = UnresolvedFlatConfig::new();
    let workflow_root = ActionId(unresolved_flat_config.entries.len() as u32);
    unresolved_flat_config.flatten_action(config.workflow, Some(workflow_root))?;

    let mut steps: Vec<_> = config.steps.into_iter().collect();
    steps.sort_by_key(|(name, _)| *name);

    let mut step_roots = HashMap::new();
    for (name, step_action) in steps {
        let step_root = unresolved_flat_config.flatten_action(step_action, None)?;
        step_roots.insert(name, step_root);
    }

    unresolved_flat_config.resolve(workflow_root, &step_roots)
}

// AFTER
pub fn flatten(config: Config) -> Result<FlatConfig, FlattenError> {
    let mut flat_config = FlatConfigBuilder::new();
    let workflow_root = ActionId(flat_config.entries.len() as u32);
    flat_config.flatten_action(config.workflow)?;

    Ok(FlatConfig {
        entries: flat_config.entries,
        handlers: flat_config.handlers,
        workflow_root,
    })
}
```

No step iteration, no resolution pass. The `workflow_root` parameter to `flatten_action` also goes away (it only existed for `Step(Root)` resolution).

#### Rust engine (`crates/barnum_engine/src/advance.rs`)

Delete `FlatAction::Step` match arm (`advance.rs:143-145`):

```rust
// DELETE
        FlatAction::Step { target } => {
            advance(workflow_state, target, value, parent)?;
        }
```

#### TypeScript AST (`libs/barnum/src/ast.ts`)

Delete `StepAction` from `Action` union (`ast.ts:11`):

```ts
// BEFORE
export type Action =
  | InvokeAction
  | ChainAction
  | ForEachAction
  | AllAction
  | BranchAction
  | StepAction
  | HandleAction
  | PerformAction;

// AFTER
export type Action =
  | InvokeAction
  | ChainAction
  | ForEachAction
  | AllAction
  | BranchAction
  | HandleAction
  | PerformAction;
```

Delete `StepAction` interface (`ast.ts:41-44`):

```ts
// DELETE
export interface StepAction {
  kind: "Step";
  step: StepRef;
}
```

Delete `StepRef` type (`ast.ts:58`):

```ts
// DELETE
export type StepRef = { kind: "Named"; name: string } | { kind: "Root" };
```

Delete `Config.steps` field (`ast.ts:128`):

```ts
// BEFORE
export interface Config<Out> {
  workflow: WorkflowAction<Out>;
  steps?: Record<string, Action>;
}

// AFTER
export interface Config<Out> {
  workflow: WorkflowAction<Out>;
}
```

#### TypeScript API (`libs/barnum/src/ast.ts`)

Delete `stepRef` function (`ast.ts:1059-1064`).

Delete `ExtractRefs` type (`ast.ts:666-670`).

Delete `ValidateStepRefs` type (`ast.ts:680-687`).

Delete `StripRefs` type (`ast.ts:1078-1083`).

Delete the entire Refs tracking comment block (`ast.ts:689-780`).

Simplify `ConfigBuilder` — remove `TSteps` type parameter, `registerSteps` method, `_buildStepRefs` helper (`ast.ts:1092-1164`):

```ts
// BEFORE
export class ConfigBuilder<TSteps extends Record<string, AnyAction> = {}> {
  private readonly _steps: Record<string, Action>;
  // registerSteps(), _buildStepRefs(), workflow({ steps, self })
}

// AFTER
export class ConfigBuilder {
  workflow<Out>(
    build: () => WorkflowAction<Out>,
  ): RunnableConfig<Out> {
    return new RunnableConfig(build());
  }
}
```

`workflow()` callback no longer receives `{ steps, self }`. It's a plain `() => WorkflowAction<Out>`.

Simplify `RunnableConfig` — remove `steps` field (`ast.ts:1216-1241`):

```ts
// BEFORE
export class RunnableConfig<Out = any> {
  readonly workflow: WorkflowAction<Out>;
  readonly steps?: Record<string, Action>;
  constructor(workflow, steps) { ... }
  toJSON(): Config<Out> { ... includes steps ... }
}

// AFTER
export class RunnableConfig<Out = any> {
  readonly workflow: WorkflowAction<Out>;
  constructor(workflow: WorkflowAction<Out>) { this.workflow = workflow; }
  toJSON(): Config<Out> { return { workflow: this.workflow }; }
}
```

#### Tests

Delete step-specific test fixtures:
- `crates/barnum_engine/tests/advance/step_named.json`
- `crates/barnum_engine/tests/completion/step_named.json`

Delete `step_named` test helper (`crates/barnum_engine/src/test_helpers.rs:57-63`).

Update TS round-trip tests (`libs/barnum/tests/round-trip.test.ts:95-117`) — remove step test cases.

Delete flattener step tests in `crates/barnum_ast/src/flat.rs` (tests using `step_named`, `config_with_steps`, `flatten_chain_of_steps`, etc.).

