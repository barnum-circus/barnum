# Handler Union Dispatch

How should pipeline nodes declare that they produce Result, Option, or other union types?

## Problem

Postfix methods like `.unwrapOr()`, `.mapErr()`, `.map()` dispatch through `__union`, which is attached to TypedAction pipeline nodes at construction time. But most ways of producing a Result/Option-typed output don't set `__union`:

- `createHandler` where the handler body returns `{ kind: "Ok", value: ... }`
- `handler.getField("foo")` where `foo` is a Result
- `branch({ A: ..., B: ... })` where a case returns a Result
- `identity()` when the input happens to be a Result

The root cause: `__union` is a property of the static pipeline graph, not of the runtime values flowing through it. TypeScript knows at compile time that `getField("foo")` returns `Result<string, string>`, but that type information doesn't exist at runtime.

### The `{ foo: Result }` problem

The problem is not specific to `createHandler`. Consider a handler that returns a struct containing a Result:

```ts
const myHandler = createHandler({
  handle: async (): Promise<{ foo: Result<string, string>; bar: number }> => {
    return { foo: { kind: "Ok", value: "hello" }, bar: 42 };
  },
});

myHandler.getField("foo")  // TypedAction<..., Result<string, string>> — __union is null
  .unwrapOr(fallback)       // RUNTIME ERROR
```

Any approach that annotates `createHandler` fails here — the Result appears only after `.getField("foo")`. This rules out handler-specific solutions as the general answer.

---

## Approach A: `Result.from(action)` — explicit wrapper

Attach `__union` to any TypedAction that produces a Result/Option, at the consumption site.

```ts
Result.from(myHandler.getField("foo")).unwrapOr(fallback)
```

Identity at the AST level — no new nodes, just attaches dispatch to the existing node.

Pros:
- Works everywhere: handlers, getField, branch, identity, any source
- Compositional — not tied to createHandler
- Type-safe — constrains the input to actually be a `Result<T, E>`
- No engine changes

Cons:
- Every Result/Option-producing action needs explicit wrapping
- Easy to forget (runtime error)

---

## Approach B: `enumKind` in the wire format — self-describing values

Embed the enum identity in the value itself:

```ts
{ kind: "Ok", enumKind: "Result", value: "nice" }
```

`Result.ok("nice")` becomes a **runtime value constructor** (not a pipeline node) that handlers use in their bodies:

```ts
handle: async (): Promise<StepResult> => Result.ok("nice")
// → { kind: "Ok", enumKind: "Result", value: "nice" }
```

The `enumKind` field travels with the value through the pipeline. Any downstream node can inspect it to determine the union family. The Rust engine threads it through (or at least doesn't strip it).

### How postfix methods work

`.unwrapOr()` no longer needs `__union` at construction time. It emits a generic AST node; the Rust engine reads `enumKind` from the runtime value to dispatch:

```ts
// TypeScript SDK — postfix method emits a generic node
function unwrapOrMethod(this: TypedAction, defaultAction: Action): TypedAction {
  return chain(toAction(this), {
    kind: "UnwrapOr",
    default_action: toAction(defaultAction),
  });
}
```

```
// Rust engine at runtime:
// 1. Reads enumKind from the input value
// 2. enumKind "Result" → Ok arm = identity, Err arm = default_action
// 3. enumKind "Option" → Some arm = identity, None arm = default_action
// 4. Dispatches based on the value's kind field
```

### The `{ foo: Result }` problem vanishes

The Result inside `foo` carries `enumKind: "Result"` in its value. After `getField("foo")`, the extracted value still has `enumKind`. No annotation needed — the value is self-describing.

### Dual meaning of `Result.ok()`

Currently `Result.ok()` is a pipeline node constructor (produces `Tag("Ok")` AST node). Under this approach, it's also (or instead) a runtime value factory used inside handler bodies. These are different things:

- **Pipeline node**: `Result.ok()` → `TypedAction` that tags a pipeline value as `Ok`
- **Runtime constructor**: `Result.ok("nice")` → plain JS object `{ kind: "Ok", enumKind: "Result", value: "nice" }`

Options:
1. **Two separate functions**: `Result.ok()` stays as the pipeline node. Add `Result.create.ok("nice")` or `Result.value.ok("nice")` as the runtime constructor.
2. **Overloaded**: `Result.ok()` (no args) returns a pipeline node. `Result.ok("nice")` (with arg) returns a runtime value. Risky — easy to confuse.
3. **Collapse to one**: `Result.ok()` is always the runtime constructor. The pipeline combinator for tagging becomes `tag("Ok")` or a new `Result.tag.ok()`. This is cleaner — the dual meaning disappears.

### Pipeline-level Tag nodes also need `enumKind`

When the pipeline (not a handler) constructs a Result — e.g., `withTimeout` tags `Ok`/`Err` — the Tag builtin in the Rust engine must also inject `enumKind`. This means the Tag AST node needs to optionally carry an `enum_kind` field:

```rust
// Current
Tag { kind: String }
// → { kind: "Ok", value: ... }

// New
Tag { kind: String, enum_kind: Option<String> }
// → { kind: "Ok", enumKind: "Result", value: ... }
```

SDK-side, `Result.ok()` (the pipeline node) would emit `Tag { kind: "Ok", enum_kind: Some("Result") }`. The plain `tag("Ok")` (no enum context) emits `Tag { kind: "Ok", enum_kind: None }` for user-defined unions that don't need dispatch.

### New AST nodes in the Rust engine

Each union operation that currently compiles to a `Branch` would instead become a dedicated AST node the engine interprets at runtime:

| TypeScript postfix | Current AST | New AST |
|-------------------|-------------|---------|
| `.unwrapOr(f)` | `Branch({ Ok: identity, Err: f })` | `UnwrapOr { default_action }` |
| `.map(f)` | `Branch({ Ok: Chain(f, Tag("Ok")), Err: Tag("Err") })` | `MapOk { action }` |
| `.mapErr(f)` | `Branch({ Ok: Tag("Ok"), Err: Chain(f, Tag("Err")) })` | `MapErr { action }` |
| `.andThen(f)` | `Branch({ Ok: f, Err: Tag("Err") })` | `AndThen { action }` |
| `.unwrap()` | `Branch({ Ok: identity, None: Panic })` | `Unwrap {}` |
| `.flatten()` | `Branch({ Some: identity, None: Tag("None") })` | `Flatten {}` |
| `.isOk()` / `.isSome()` | `Branch({ Ok: Constant(true), Err: Constant(false) })` | `IsOkVariant {}` |

The engine reads `enumKind` to determine which `kind` values map to the "success" and "failure" arms. This mapping is a small lookup table: `Result → { success: "Ok", failure: "Err" }`, `Option → { success: "Some", failure: "None" }`.

### Branch still works as-is

`branch({ Ok: ..., Err: ... })` continues to match on exact `kind` strings. No change needed. `enumKind` is orthogonal to branch — branch doesn't need to know the family, it just matches the variant.

Pros:
- Eliminates `__union` entirely from the TypeScript SDK
- The `{ foo: Result }` problem vanishes — values are self-describing
- No annotation needed anywhere — handlers return values with `enumKind`, it just works
- Clean separation: SDK constructs generic nodes, engine handles dispatch
- `enumKind` is visible in JSON logs/traces — good for debugging

Cons:
- **Rust engine changes.** New AST node variants, `enumKind` threading, enum family lookup table.
- **Wire format change.** Values gain an `enumKind` field. Existing handlers returning bare `{ kind: "Ok", value: ... }` would need to add `enumKind` (or use runtime constructors). Migration cost.
- **Dual `Result.ok()` meaning** needs resolution (options above).
- **Tag builtin change.** Needs optional `enum_kind` field.
- **User-defined unions** need a way to register their enum family with the engine (kind→arm mapping).

---

## Summary

| Approach | Solves `{ foo: Result }`? | Needs annotation? | Engine changes? | Wire format change? |
|----------|--------------------------|-------------------|-----------------|---------------------|
| A. `Result.from()` | Yes (explicit) | Yes, every time | None | None |
| B. `enumKind` in values | Yes (automatic) | No | Yes (new AST nodes + `enumKind` threading) | Yes (`enumKind` field) |

### Open questions

1. Which resolution for the `Result.ok()` dual meaning? Separate functions, overloading, or collapse to runtime-only?
2. How do user-defined unions register their enum family? A `defineEnum("MyEnum", { Success: ..., Failure: ... })` API?
3. Should `enumKind` be required on all tagged union values, or only on Result/Option? If required, plain `tag("Foo")` would need an enum context too. If optional, values without `enumKind` can't use postfix methods (which is the current behavior, just explicit).
4. Is the migration cost acceptable? Every handler returning `{ kind: "Ok", value: ... }` needs to switch to `Result.ok(...)` or add `enumKind` manually.
