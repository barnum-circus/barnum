# Rust-Inspired Combinators for Barnum

How to bring Rust's Option, Result, Iterator, and combinator patterns into the workflow algebra.

> **Convention**: All discriminated unions use `TaggedUnion<Def>` — every variant carries `{ kind: K; value: T; __def?: Def }`. All union constructors (`tag`, `recur`, `done`, `some`, `none`) require the full variant map so output carries `__def`. Branch auto-unwraps `value` — case handlers receive the payload directly.

## Option<T>

```ts
type OptionDef<T> = { Some: T; None: void };
type Option<T> = TaggedUnion<OptionDef<T>>;
```

This is a tagged union — `branch` dispatches on it naturally. Branch auto-unwraps `value`, so `Some` handler receives `T` directly and `None` handler receives `void`.

### Constructors

- `some()` = `tag<OptionDef<T>, "Some">("Some")` — tag knows the full union
- `none()` = produces `{ kind: "None"; value: undefined }` — fixed value

### Combinators

Branch auto-unwraps `value`, so no `extractField("value")` needed in implementations:

| Rust | Barnum | Implementation |
|------|--------|----------------|
| `.map(f)` | `mapOption(action)` | `branch({ Some: pipe(action, tag<OptionDef<U>, "Some">("Some")), None: identity() })` |
| `.and_then(f)` | `flatMapOption(action)` | `branch({ Some: action, None: identity() })` — action must return Option |
| `.unwrap_or(default)` | `unwrapOptionOr(default)` | `branch({ Some: identity(), None: default })` — `default` is an action |
| `.unwrap()` | `unwrap()` | `branch({ Some: identity(), None: panic("unwrap on None") })` — requires error handling |
| `.is_some()` | N/A | `branch({ Some: constant(true), None: constant(false) })` |
| `.or(other)` | `optionOr(other)` | `branch({ Some: identity(), None: other })` |
| `.filter(pred)` | Hard — requires expression evaluation in AST |

### Naming convention

Include "option" in every name: `mapOption`, `flatMapOption`, `unwrapOptionOr`, `optionOr`. This avoids collision with potential Result variants and makes the semantics obvious at the call site.

### `unwrapOr` takes an action, not a raw value

`unwrapOr(constant("anonymous"))`, not `unwrapOr("anonymous")`. The default is an AST node — this keeps the combinator composable (the default can be a computation, not just a literal).

### Postfix operators with `this` constraints

The highest-value Option combinators should be postfix methods on TypedAction, gated by a `this` parameter constraint so they're only callable when `Out` matches the Option shape. See POSTFIX_OPERATORS.md § "Option/Result postfix operators (Phase 2)".

```ts
action.mapOption(transform)    // only available when Out is Option<T>
action.unwrapOptionOr(default) // only available when Out is Option<T>
action.optionOr(fallback)      // only available when Out is Option<T>
```

### Which to provide as builtins?

`mapOption`, `flatMapOption`, and `unwrapOptionOr` are the highest value. Each saves 3+ lines of branch/extractField boilerplate. The rest are one-liners over `branch`. Provide as both prefix functions and postfix methods (gated by `this` constraint).

## Result<T, E>

```ts
type ResultDef<T, E> = { Ok: T; Err: E };
type Result<T, E> = TaggedUnion<ResultDef<T, E>>;
```

Produced by `tryAction(handler)` (see MISSING_LANGUAGE_FEATURES.md).

### Combinators

Branch auto-unwraps `value` — `Ok` handler receives `T`, `Err` handler receives `E`:

| Rust | Barnum | Implementation |
|------|--------|----------------|
| `.map(f)` | `mapOk(action)` | `branch({ Ok: pipe(action, tag<ResultDef<U, E>, "Ok">("Ok")), Err: identity() })` |
| `.map_err(f)` | `mapErr(action)` | `branch({ Ok: identity(), Err: pipe(action, tag<ResultDef<T, F>, "Err">("Err")) })` |
| `.and_then(f)` | `flatMapOk(action)` | `branch({ Ok: action, Err: identity() })` — action must return Result |
| `.unwrap()` | `unwrapOk()` | `branch({ Ok: identity(), Err: panic("unwrap on Err") })` |
| `.unwrap_or(default)` | `unwrapOkOr(default)` | `branch({ Ok: identity(), Err: default })` — `default` is an action |
| `?` operator | `scope` + `exit` | See LOOP_WITH_CLOSURE.md — `done()` / `exit()` is exactly `?` |

### The `?` operator

This is the killer feature. In Rust, `?` propagates errors up to the enclosing function. In Barnum, `scope` + `exit` does the same thing. Branch auto-unwraps, so the `Ok` handler receives the value directly:

```ts
scope(({ exit }) =>
  pipe(
    tryAction(step1),
    branch({ Ok: identity(), Err: exit() }),  // ? operator — Ok unwraps, Err exits scope
    tryAction(step2),
    branch({ Ok: identity(), Err: exit() }),
    tryAction(step3),
    branch({ Ok: identity(), Err: exit() }),
  ),
)
// output: last Ok value, or first Err value
```

Sugar: `propagate()` = `branch({ Ok: identity(), Err: exit() })`. Then:

```ts
scope(({ exit }) =>
  pipe(
    tryAction(step1), propagate(exit),
    tryAction(step2), propagate(exit),
    tryAction(step3), propagate(exit),
  ),
)
```

Or even: `tryScope` as a primitive that automatically propagates Err:

```ts
tryScope(
  pipe(step1, step2, step3),  // each step can "throw" by producing Err
)
```

## Iterator / ForEach patterns

`forEach` is Barnum's `map`. What about the rest of the iterator toolkit?

| Rust | Barnum | Status |
|------|--------|--------|
| `.map(f)` | `forEach(action)` | Exists |
| `.filter(pred)` | `filter(action)` | New — action returns boolean, scheduler drops false elements |
| `.filter_map(f)` | `filterMap(action)` | New — action returns Option, collect Somes |
| `.flat_map(f)` | `pipe(forEach(action), flatten())` | Exists via composition |
| `.collect()` | Implicit — `forEach` already collects | Exists |
| `.fold(init, f)` | No general fold (see MISSING_LANGUAGE_FEATURES.md) | Hard |
| `.zip(other)` | `parallel(forEach(a), forEach(b))` then element-wise pair | Awkward |
| `.enumerate()` | `enumerate()` | New — adds index: `[T] → [{ index: number, value: T }]` |
| `.take(n)` | `take(n)` | New — builtin to slice first N elements |
| `.skip(n)` | `skip(n)` | New — builtin to skip first N elements |
| `.any(pred)` / `.all(pred)` | Compose with forEach + branch | Possible |
| `.chain(other)` | Array concatenation builtin | New |
| `.count()` | `count()` | New — builtin returning array length |

### Which to provide?

**High value**: `filter` and `filterMap` (= `collectSome`). These come up constantly — "process each file, skip failures."

**Medium value**: `enumerate`, `count`, `take`, `skip`. Common array operations.

**Low value**: `zip`, `fold`, `any`/`all`. Rarely needed in workflows.

### filter implementation

`filter` requires the action to return a boolean, and the scheduler drops elements where the result is false. This is a new AST node:

```ts
{ kind: "Filter", predicate: Action }
```

Alternative: `filterMap` is more general. Have the predicate return `Option<T>`, and the scheduler collects `Some` values. This avoids the boolean problem (booleans aren't tagged unions).

```ts
forEach(
  pipe(
    tryAction(processFile),
    branch({
      Ok: some(),     // auto-unwraps Ok value, wraps in Some
      Err: pipe(logError, none()),
    }),
  ),
).then(collectSome())
```

## From / Into (type conversions)

Rust's `From`/`Into` traits enable implicit conversions. In Barnum, there are no implicit conversions — everything is explicit `pipe(extractField(...), tag(...))`. This is correct for a workflow DSL. Implicit conversions would be confusing.

**Skip this.** Explicit data shaping is a feature, not a bug.

## collect() patterns

Rust's `collect()` uses `FromIterator` to collect into different container types. Barnum's `forEach` always collects into an array. Other collection patterns:

- **collectSome**: `Option<T>[] → T[]` — drop Nones, unwrap Somes
- **collectOk**: `Result<T, E>[] → T[]` — drop Errs, unwrap Oks
- **partition**: `Result<T, E>[] → { ok: T[], err: E[] }` — split into successes and failures

`partition` is particularly useful: "run N tasks in parallel, handle successes and failures separately."

## Priority

1. **`tryAction` + `scope`/`exit` (the ? operator)** — error handling is the biggest gap
2. **`mapOption`, `flatMapOption`, `unwrapOptionOr`** — Option combinators used constantly
3. **`collectSome` / `filterMap`** — filtering is fundamental to iteration
4. **`mapOk`, `mapErr`, `propagate`** — Result combinators once tryAction exists
5. **`partition`** — parallel error handling
6. **`enumerate`, `count`, `take`, `skip`** — array utilities
