# Second-Class Functions

## Motivation

Some functions should not be allowed to escape the scope that created them. A "resume" callback provided by a Handle block, a file-write capability granted by a sandbox, a resource accessor tied to an RAII scope — all of these are meaningful only within their defining scope. If they leak out, they become dangling references to dead infrastructure.

Second-class functions are functions that can be passed down (into child scopes) but not returned up (from enclosing scopes). They enforce a stack discipline: the function cannot outlive the scope that created it.

This is distinct from RAII. RAII ensures cleanup runs on scope exit. Second-class functions ensure that references to scoped capabilities don't escape in the first place. They're complementary: RAII cleans up resources, second-class restrictions prevent dangling references to those resources.

## What "second-class" means concretely

A first-class function can be:
- Called
- Passed as an argument
- Stored in a data structure
- Returned from a function

A second-class function can be:
- Called
- Passed as an argument (downward only)

It cannot be returned or stored in a structure that outlives its scope. The type system (or runtime) enforces that the function's lifetime is bounded by its creating scope.

## Where second-class functions appear in this system

### 1. Effect handler capabilities

A Handle block intercepts effects and can provide capabilities to its body. If the body gets a "perform this effect" function, that function is only valid while the Handle is active. Returning it would produce a function that dispatches to a dead handler.

```ts
handle(myEffect, handler,
  (perform) => pipe(
    perform(payload),  // OK: calling within scope
    passToChild,       // OK: child receives perform, uses it, doesn't return it
  )
)
```

`perform` is second-class: the body and its children can call it, but the pipeline's output must not contain it.

### 2. Resource accessors (RAII interaction)

When a Bracket scope acquires a resource, it may expose an accessor function. That accessor is valid only while the resource is alive. If the accessor escapes the Bracket scope, it references a disposed resource.

```ts
bracket(createConnection, disposeConnection,
  (getConn) => pipe(
    getConn(),         // OK: use within scope
    query("SELECT 1"),
  )
)
```

`getConn` is second-class. The query result (a plain value) can escape; the accessor cannot.

### 3. Scoped callbacks from `declare`/`provider`

A provider that injects a callback (e.g., "call this to log an audit event") into the body DAG. The callback routes to the provider's handler. If it escapes the provider scope, it dispatches to nothing.

## Enforcement strategies

### Static (type-level)

Tag second-class functions with a lifetime or scope marker in the type system. The TypeScript builder rejects pipelines that return a type containing a second-class marker. This is the Rust approach (lifetimes prevent references from escaping their borrow scope).

In TypeScript's type system, this could look like:

```ts
// Phantom type that marks second-class values
type ScopedTo<TScope, TValue> = TValue & { __scope: TScope };

// The builder tracks that ScopedTo<S, T> cannot appear in the output type
// of an action whose enclosing scope is S.
```

Whether TypeScript's type system is expressive enough to enforce this without escape hatches is an open question. Rust lifetimes work because the compiler has first-class support for them. TypeScript phantom types are advisory — a cast bypasses them.

### Dynamic (runtime)

The scheduler tags second-class values with their scope ID. When a scope exits, any value carrying that scope's tag becomes invalid. If a downstream action receives an invalidated value, the scheduler raises an error.

Simpler to implement than static enforcement and catches violations at runtime rather than build time. The cost is that errors are late (runtime, not compile time) and the tagging has overhead.

### Hybrid

The builder warns at build time using best-effort type checking. The scheduler enforces at runtime as a safety net. This matches how the system already works: the TypeScript layer catches most errors statically, but the Rust scheduler validates invariants at runtime.

## Relation to other pending work

- **RAII.md**: RAII cleans up resources on scope exit. Second-class functions prevent references to those resources from escaping. Without second-class enforcement, a Bracket scope can dispose a resource while an escaped accessor still references it.
- **EFFECTS_DEFERRED.md (Capabilities)**: The capabilities section describes Handle blocks as capability grants. Second-class functions are the enforcement mechanism — the capability is a second-class function scoped to the Handle block.
- **EFFECTS_PHASE_5_ADVANCED.md (Bracket)**: Bracket exposes resource accessors that are naturally second-class. The current design doesn't enforce this; adding second-class semantics would close the hole.

## Open questions

1. **Is static enforcement feasible in TypeScript?** Phantom types can model it, but TypeScript's structural type system and `any` casts provide escape hatches. If static enforcement is leaky, is it worth the type complexity, or should we rely on runtime enforcement alone?

2. **Granularity of scope markers.** Should every Handle, Bracket, and Provider create a distinct scope, or should there be explicit `scoped` boundaries that the user opts into? Finer granularity is safer but adds noise.

3. **Interaction with `all` and variable sharing.** If a second-class function is passed to multiple branches of an `all`, each branch can use it, but none can return it. The `all` node's output type must not contain it. This is straightforward in principle but requires the type machinery to track second-class markers through combinators.

4. **Should plain values derived from second-class functions be restricted?** If `getConn()` returns a connection handle (a plain value, not a function), should that handle also be second-class? The connection handle is just as dangling as the accessor after dispose. This pushes toward linear/affine types for resource handles, which is a larger design space than second-class functions alone.
