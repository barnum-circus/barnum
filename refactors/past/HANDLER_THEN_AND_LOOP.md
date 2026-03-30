# Handler `.then()` and `.forEach()`

**Status:** Pending

**Depends on:** HANDLER_CONFIG_DESUGARING.md (handlers must be `TypedAction` objects, not callable functions)

## Motivation

After config desugaring lands, `createHandler` returns a `TypedAction` directly. Composition still uses free functions:

```ts
pipe(initialize, build, deploy, report)
```

Adding `.then()` and `.forEach()` methods on `TypedAction` enables method chaining as an alternative:

```ts
initialize.then(build).then(deploy).then(report)
```

Both styles produce identical AST nodes. Method chaining reads better for linear pipelines; free functions read better for fanout (`all`) and branching (`branch`).

## Design

### `TypedAction` becomes a class

`TypedAction` is currently a structural type (intersection of `Action` with phantom fields). It has no methods. To add methods, it becomes a class:

```ts
class ActionNode<In = unknown, Out = unknown, Refs extends string = never> {
  readonly action: Action;

  declare __phantom_in?: (input: In) => void;
  declare __phantom_out?: () => Out;
  declare __in?: In;
  declare __refs?: { _brand: Refs };

  constructor(action: Action) {
    this.action = action;
  }

  then<Next, R2 extends string>(
    next: ActionNode<Out, Next, R2>,
  ): ActionNode<In, Next, Refs | R2> {
    return new ActionNode({ kind: "Chain", first: this.action, rest: next.action });
  }

  forEach(): ActionNode<In[], Out[], Refs> {
    return new ActionNode({ kind: "ForEach", action: this.action });
  }

  toJSON(): Action {
    return this.action;
  }
}
```

### `.then()` produces Chain

`a.then(b)` is `chain(a, b)`. Output type of `a` must match input type of `b`.

### `.forEach()` lifts to ForEach

`action.forEach()` wraps the action in a ForEach node. Lifts `ActionNode<A, B>` to `ActionNode<A[], B[]>`. Always valid.

### Serialization

`toJSON()` returns the plain `Action`. `JSON.stringify` calls this automatically, so `ActionNode` serializes identically to the current structural `TypedAction`. The Rust side never sees the wrapper.

### Free functions remain

`pipe()`, `forEach()`, `loop()`, `all()`, `branch()` still work. They can delegate to methods internally:

```ts
export function pipe<T1, T2, T3>(
  a1: ActionNode<T1, T2>,
  a2: ActionNode<T2, T3>,
): ActionNode<T1, T3> {
  return a1.then(a2);
}
```

`all()` and `branch()` stay as free functions only -- they don't have natural method syntax.

## Usage examples

```ts
// Linear pipeline
initialize.then(build).then(deploy).then(report)

// ForEach
listFiles.then(processFile.forEach())

// Handler with config (from HANDLER_CONFIG_DESUGARING.md)
initialize.then(deploy({ target: "production" })).then(report)

// Mixed: free functions where they read better
initialize.then(
  all(checkHealth, notify, report)
)
```

## Open questions

1. **Replace `TypedAction` entirely or coexist?** Replacing avoids a confusing duality. Since backward compatibility is not a concern, replacing is cleaner.

2. **`createHandler` returns `ActionNode`.** The handler object would be an `ActionNode` instance with `__definition` attached (non-enumerable). The `toJSON()` method handles serialization.

3. **`pipe()` overloads.** The 10 existing overloads (2-10 args) are still useful for multi-step pipelines where method chaining is noisier. They delegate to `.then()` internally.
