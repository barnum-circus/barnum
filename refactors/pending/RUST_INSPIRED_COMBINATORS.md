# Rust-Inspired Combinators for Barnum

How to bring Rust's Option, Result, Iterator, and combinator patterns into the workflow algebra.

> **Convention**: All discriminated unions use `TaggedUnion<Def>` — every variant carries `{ kind: K; value: T; __def?: Def }`. All union constructors (`tag`, `recur`, `done`, `some`, `none`) require the full variant map so output carries `__def`. Branch auto-unwraps `value` — case handlers receive the payload directly.

## Option<T>

See **OPTION_TYPES.md** for the comprehensive Option combinator library (all Rust Option methods mapped to barnum, namespaced as `Option.map()`, `Option.andThen()`, etc.).

Summary: `Option<T> = TaggedUnion<{ Some: T; None: void }>`. All combinators live on an `Option` namespace object. Postfix methods gated by `this` constraint on TypedAction.

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
