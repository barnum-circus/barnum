# Handler `.then()` and Loop Ergonomics

**Status:** Pending

**Depends on:** Removing `stepConfig` from handlers (see below)

## Motivation

The current API for composing workflows uses free functions:

```ts
pipe(initialize(), build(), deploy(), report())
```

Each handler is a callable function (created by `createHandler`), and the function call `build()` produces a `TypedAction`. Handlers are functions solely because `stepConfig` needs a call site:

```ts
build({ stepConfig: { target: "production" } })
```

Without `stepConfig`, `build()` is a zero-argument function call that always produces the same AST node. That's unnecessary ceremony. Handlers could just *be* `TypedAction` values directly, with methods for composition.

## Two changes

This document covers two intertwined changes:

1. **Remove `stepConfig` from handlers**, so handlers become `TypedAction` objects instead of functions.
2. **Add `.then()`, `.forEach()`, and `.loop()` methods** on `TypedAction`, replacing the need for `pipe()`, `forEach()`, and `loop()` free functions for common cases.

These are the same change in the sense that (1) is a prerequisite for (2) to feel right. Calling `build().then(deploy())` is awkward when `build()` is a nullary function that exists only because stepConfig once required a call site.

## Part 1: Remove `stepConfig`

### Current state

`createHandler` accepts an optional `stepConfigValidator` and returns a `CallableHandler` -- a function that takes `{ stepConfig?: TStepConfig }` and returns `TypedAction`:

```ts
// handler.ts:65-72
export type CallableHandler<TValue, TOutput, TStepConfig> =
  ((options?: { stepConfig?: TStepConfig }) => TypedAction<TValue, TOutput>)
  & Handler<TValue, TOutput, TStepConfig>;
```

The Rust AST carries `step_config_schema: Option<Value>` on `TypeScriptHandler`. The engine doesn't interpret it -- it passes the value through to the TS worker, which feeds it to the handler as `context.stepConfig`.

### What to do instead

The `stepConfig` use case is "inject a static value alongside the pipeline value." The existing combinators already express this:

```ts
// Before (stepConfig):
build({ stepConfig: { target: "production" } })

// After (explicit AST):
pipe(
  parallel(identity(), constant({ target: "production" })),
  merge(),
  buildHandler,  // receives { value: PipelineValue, target: "production" }
)
```

This is verbose, so `createHandler` (or a helper) should emit the desugaring automatically when the handler has a config:

```ts
// build.ts
export default createHandler({
  stepValueValidator: z.object({ artifact: z.string() }),
  handle: async ({ value }) => { ... },
});

// build-with-config.ts -- handler that needs config
export default createHandler({
  stepValueValidator: z.object({ artifact: z.string() }),
  stepConfigValidator: z.object({ target: z.string() }),
  handle: async ({ value }) => {
    // value is { artifact: string } & { target: string }
    // merged from pipeline value + config
  },
});
```

When a handler has a `stepConfigValidator`, the handler *object* carries that information, and calling a method like `.withConfig({ target: "production" })` emits the `parallel(identity(), constant(config)) + merge + invoke` subtree. The handler itself (the Invoke node) always takes a single merged value.

### Rust-side changes

Remove `step_config_schema` from `TypeScriptHandler`. The Rust AST `HandlerKind` becomes:

```rust
pub struct TypeScriptHandler {
    pub module: ModulePath,
    pub func: FuncName,
}
```

The worker (`worker.ts`) stops reading `stepConfig` from the invocation envelope. Handlers receive `{ value }` only.

### TS worker changes

The worker currently does:

```ts
const result = await handler.__definition.handle({ value: input.value });
```

It already doesn't pass stepConfig (the current worker implementation ignores it). No change needed.

### Handler type simplification

`Handler` drops from 3 type parameters to 2:

```ts
// Before:
export type Handler<TValue, TOutput, TStepConfig>

// After:
export type Handler<TValue, TOutput>
```

`HandlerDefinition` becomes:

```ts
export type HandlerDefinition<TValue = unknown, TOutput = unknown> = {
  stepValueValidator?: z.ZodType<TValue>;
  handle: (context: { value: TValue }) => Promise<TOutput>;
};
```

`CallableHandler` is deleted. `createHandler` returns a `Handler` that is also a `TypedAction`.

## Part 2: Handler as TypedAction with methods

### Current state

`TypedAction` is a structural type (intersection of `Action` with phantom fields). It has no methods. Composition uses free functions:

```ts
pipe(a, b, c)
forEach(action)
loop(body)
```

Handlers are callable functions, not `TypedAction` values:

```ts
const action: TypedAction = build();  // call to get the action
pipe(action, deploy());               // then compose
```

### Target

`createHandler` returns an object that IS a `TypedAction` and has composition methods:

```ts
import build from "./handlers/build.js";
import deploy from "./handlers/deploy.js";

// build is already a TypedAction, no () needed
build.then(deploy).then(report)
```

### Implementation approach: wrapper class

`TypedAction` is currently a structural type, which can't have methods. We need a class:

```ts
class ActionNode<In = unknown, Out = unknown, Refs extends string = never> {
  /** The underlying serializable action. */
  readonly action: Action;

  // Phantom fields for type tracking (same as current TypedAction)
  declare __phantom_in?: (input: In) => void;
  declare __phantom_out?: () => Out;
  declare __in?: In;
  declare __refs?: { _brand: Refs };

  constructor(action: Action) {
    this.action = action;
  }

  /** Chain: run this, then next. */
  then<Next, R2 extends string>(
    next: ActionNode<Out, Next, R2>,
  ): ActionNode<In, Next, Refs | R2> {
    return new ActionNode({ kind: "Chain", first: this.action, rest: next.action });
  }

  /** ForEach: apply this action to each element of an array input. */
  forEach(): Out extends unknown[]
    ? ActionNode<In extends unknown[] ? In[number] : never, Out[number], Refs>  // ← wrong
    : never {
    // ...
  }

  /** Loop: repeat this action until it signals Break. */
  loop(): ActionNode</* ... */> {
    return new ActionNode({ kind: "Loop", body: this.action });
  }

  /** Serialize to JSON (used by JSON.stringify). */
  toJSON(): Action {
    return this.action;
  }
}
```

**The `toJSON()` method is critical.** `JSON.stringify` calls `toJSON()` on objects, so `ActionNode` serializes identically to a plain `Action`. The Rust side never sees the wrapper.

### The `.forEach()` type problem

`.forEach()` lifts `ActionNode<A, B>` to `ActionNode<A[], B[]>`. The constraint is on the call site, not on the action's types. Every action can be lifted -- the question is whether the result is useful.

```ts
forEach(): ActionNode<In[], Out[], Refs> {
  return new ActionNode({ kind: "ForEach", action: this.action });
}
```

The signature is straightforward. We previously discussed constraining `.forEach()` to only type-check when the output is an array. That was for a different scenario (checking the *output* is an array so you can map over its elements). Here, `.forEach()` wraps the action to operate on arrays of its input/output. No constraint needed -- it's always valid.

### The `.loop()` type constraint

`.loop()` wraps the action in a `Loop`. The action must return `LoopResult<TContinue, TBreak>` where `TContinue` matches the action's input. The constraint:

```ts
loop<TContinue, TBreak>(
  this: ActionNode<TContinue, LoopResult<TContinue, TBreak>, Refs>,
): ActionNode<TContinue, TBreak, Refs> {
  return new ActionNode({ kind: "Loop", body: this.action });
}
```

Using `this` parameter to constrain the types. This only type-checks when `Out` is `LoopResult<In, TBreak>`.

### The `.then()` overload for handlers

When `.then()` receives a bare `Handler` (not wrapped in `ActionNode`), it should work:

```ts
then<Next, R2 extends string>(
  next: ActionNode<Out, Next, R2> | Handler<Out, Next>,
): ActionNode<In, Next, Refs | R2> {
  const nextAction = next instanceof ActionNode ? next : next.toActionNode();
  return new ActionNode({ kind: "Chain", first: this.action, rest: nextAction.action });
}
```

Or if handlers ARE `ActionNode` instances, no overload is needed.

### `createHandler` returns `ActionNode`

```ts
export function createHandler<TValue, TOutput>(
  definition: HandlerDefinition<TValue, TOutput>,
  exportName?: string,
): ActionNode<TValue, TOutput> & { __definition: HandlerDefinition<TValue, TOutput> } {
  const filePath = getCallerFilePath();
  const funcName = exportName ?? "default";
  const action: Action = {
    kind: "Invoke",
    handler: { kind: "TypeScript", module: filePath, func: funcName },
  };
  const node = new ActionNode<TValue, TOutput>(action);
  // Attach definition for worker runtime
  (node as any).__definition = definition;
  return node as any;
}
```

The returned object is an `ActionNode` that also carries `__definition` for the worker to call at runtime.

### Config builder interaction

`ConfigBuilder.workflow()` currently accepts `TypedAction<never, Out>`. It would accept `ActionNode<never, Out>` instead. The `toJSON()` method on `ActionNode` makes serialization transparent.

### Free functions remain available

`pipe()`, `forEach()`, `loop()`, etc. still work. They wrap their results in `ActionNode`:

```ts
export function pipe<T1, T2, T3>(
  a1: ActionNode<T1, T2>,
  a2: ActionNode<T2, T3>,
): ActionNode<T1, T3> {
  return a1.then(a2);
}
```

Method chaining and free functions are interchangeable. Users pick whichever reads better for their workflow.

## Usage examples

### Simple pipeline

```ts
// Free functions (current):
pipe(initialize(), build(), deploy(), report())

// Method chaining (new):
initialize.then(build).then(deploy).then(report)
```

### ForEach

```ts
// Free functions:
pipe(listFiles(), forEach(processFile()))

// Method chaining:
listFiles.then(processFile.forEach())
```

### Loop

```ts
// Free functions:
pipe(startPolling(), loop(pollStatus()))

// Method chaining:
startPolling.then(pollStatus.loop())
```

### Handler with config

```ts
// Handler needs extra static config:
const buildProd = build.withConfig({ target: "production" });

// Desugars to: parallel(identity, constant({target:"production"})) + merge + build
// But the user just sees:
initialize.then(buildProd).then(deploy)
```

## Serialization

`ActionNode` must serialize to the same JSON as bare `Action`. Two mechanisms:

1. **`toJSON()`** on `ActionNode` returns the plain `Action`. `JSON.stringify` calls this.
2. **Config builder** calls `toJSON()` explicitly when building the config object.

The Rust deserializer never sees `ActionNode`. It gets the same `Action` tree it always has.

## Open questions

1. **Should `ActionNode` replace `TypedAction` entirely, or coexist?** If it replaces `TypedAction`, all combinator signatures change. If it coexists, there's a confusing duality. Replacing it cleanly is preferable since backward compatibility is not a concern.

2. **The `withConfig()` method.** Should this live on `ActionNode` (available on all actions, but only makes sense on Invoke), or only on the handler subtype? A handler-specific subclass (`HandlerNode extends ActionNode`) could carry `withConfig` while plain `ActionNode` doesn't have it.

3. **`parallel()` and `branch()` as methods?** `a.parallel(b, c)` doesn't make sense syntactically. These stay as free functions. Method chaining covers the sequential (`.then()`) and lifting (`.forEach()`, `.loop()`) cases. Fanout and branching remain free functions.

4. **The `pipe()` overloads.** Currently there are 10 overloads (2-10 args) for type inference. If `.then()` exists, are the overloads still needed? Probably yes -- `pipe(a, b, c, d)` is less noisy than `a.then(b).then(c).then(d)` in some contexts. The overloads can delegate to `.then()` internally.
