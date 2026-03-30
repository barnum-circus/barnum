# Handle state_init

## Goal

Add a `state_init` field to `HandleAction` so the Handle frame's initial state can be derived from the pipeline value rather than being the raw pipeline value itself.

## Motivation

When a Handle frame is created, it needs an initial `state` value. Currently, Handle has no mechanism for this — `HandleFrame.state` starts as `None`. With `state_init`, the Handle can run an expression on the pipeline input to extract or transform it before storing it as state.

This is needed by `declare` (Phase 2), where each Handle stores one binding's value extracted from a tuple:

```
// Pipeline value entering Handle: [User, Config, pipeline_input]
// state_init: ExtractIndex(0)
// Initial state: User
```

But `state_init` is independently useful for any effect handler that needs to derive its initial state from context.

## Changes

### AST

Add `state_init` to `HandleAction`:

```rust
pub struct HandleAction {
    pub effect_id: EffectId,
    pub body: Box<Action>,
    pub handler: Box<Action>,
    pub state_init: Option<Box<Action>>,  // NEW
}
```

When `state_init` is `None`, the frame's initial state is the raw pipeline value (backward-compatible default). When `Some`, the engine evaluates the action on the pipeline value and uses the result as initial state.

### Engine

In `advance()`, when creating a Handle frame:

1. If `state_init` is `None`: set `frame.state = Some(pipeline_value)`
2. If `state_init` is `Some(action)`: evaluate `action` on `pipeline_value`, set `frame.state = Some(result)`

The `state_init` action is a leaf expression (e.g. `ExtractIndex(0)`) — it doesn't create its own frame. It's evaluated inline during frame creation, same as how builtins are evaluated inline during `advance()`.

### TypeScript AST mirror

Add the field to the TS `Action` type and the `HandleAction` interface (once Handle exists in the TS AST — Phase 1 adds it).

## Deliverables

1. Add `state_init: Option<Box<Action>>` to `HandleAction` in `barnum_ast`
2. Evaluate `state_init` during Handle frame creation in the engine
3. Update serialization/deserialization (serde handles `Option` as absent-when-None)
4. Test: Handle with `state_init = ExtractIndex(0)` on input `[42, "hello"]` → state = `42`
5. Test: Handle with `state_init = None` on input `"raw"` → state = `"raw"`
