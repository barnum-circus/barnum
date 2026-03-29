# Namespacing: organizing the barnum API surface

## Current top-level exports

Everything is currently a flat top-level export. Full inventory:

### AST combinators (produce AST nodes)

| Export | From | AST node | Description |
|--------|------|----------|-------------|
| `pipe(...)` | `pipe.ts` | `Chain` | Sequential composition |
| `chain(a, b)` | `chain.ts` | `Chain` | Binary chain (pipe is n-ary sugar) |
| `parallel(...)` | `parallel.ts` | `Parallel` | Fork input to N branches, collect tuple |
| `forEach(action)` | `ast.ts` | `ForEach` | Map action over array |
| `branch(cases)` | `ast.ts` | `Branch` | Dispatch on tagged union |
| `loop(body)` | `ast.ts` | `Loop` | Repeat until Break |

### Builtin handlers (Invoke + BuiltinKind)

| Export | From | BuiltinKind | Description |
|--------|------|-------------|-------------|
| `constant(value)` | `builtins.ts` | `Constant` | Produce fixed value |
| `identity()` | `builtins.ts` | `Identity` | Pass through |
| `drop()` | `builtins.ts` | `Drop` | Discard value |
| `tag(kind)` | `builtins.ts` | `Tag` | Wrap as tagged union variant |
| `recur()` | `builtins.ts` | `Tag("Continue")` | Loop continue signal |
| `done()` | `builtins.ts` | `Tag("Break")` | Loop break signal |
| `merge()` | `builtins.ts` | `Merge` | Merge tuple of objects |
| `flatten()` | `builtins.ts` | `Flatten` | Flatten nested array |
| `extractField(field)` | `builtins.ts` | `ExtractField` | Get object field |
| `extractIndex(index)` | `builtins.ts` | `ExtractIndex` | Get array element |
| `pick(...keys)` | `builtins.ts` | `Pick` | Select object fields |

### TypeScript-only combinators (compose AST, no dedicated node)

| Export | From | Description |
|--------|------|-------------|
| `dropResult(action)` | `builtins.ts` | Run action, discard output (= `chain(action, drop())`) |
| `augment(action)` | `builtins.ts` | Run action, merge output into input (= `parallel(action, identity()) → merge()`) |
| `tap(action)` | `builtins.ts` | Side effect, preserve input (= `parallel(chain(action, constant({})), identity()) → merge()`) |
| `withResource(...)` | `builtins.ts` | RAII create/action/dispose pattern |
| `range(start, end)` | `builtins.ts` | Produce `[start..end)` array (= `constant([...])`) |

### Postfix methods on TypedAction

`.then()`, `.forEach()`, `.branch()`, `.flatten()`, `.drop()`, `.tag()`, `.get()`, `.augment()`, `.pick()`

## What should be namespaced?

### Already designed namespaces

- **`Option`** — `some`, `none`, `map`, `andThen`, `unwrapOr`, `or`, `collect`, etc. (OPTION_TYPES.md)
- **`LoopResult`** — `recur`, `done`, `mapBreak`, `mapContinue`, etc. (CONTROL_FLOW.md)
- **`Result`** — `ok`, `err`, `map`, `mapErr`, `andThen`, `unwrapOr`, etc. (RESULT.md)

These namespace tagged union operations by their type. Makes sense — `Option.map` is unambiguous, `map` alone is not.

### Candidates for namespacing

#### `tag` → specific namespaces only?

`tag<TDef, TKind>(kind)` is the generic constructor. It's used internally by `Option.some`, `Result.ok`, `LoopResult.recur`, etc. Question: should `tag` remain a top-level export for ad-hoc unions, or only be available through specific namespaces?

**Keep top-level.** Ad-hoc unions are common in handlers:

```ts
// Handler returns { kind: "HasErrors"; value: ... } | { kind: "Clean"; value: ... }
// No predefined namespace for this union
tag<ClassifyResultDef, "HasErrors">("HasErrors")
```

Removing `tag` forces every union to have a namespace object, which is overkill for one-off types.

#### `recur` / `done` → `LoopResult` namespace only?

Currently top-level. After the `LoopResult` namespace exists: `LoopResult.recur()` and `LoopResult.done()`.

**Deprecate top-level, keep for backward compat during transition.** The namespace form is strictly better — it groups loop signals with their type. Top-level `recur()` is confusing without context.

Long-term: remove top-level `recur`/`done` entirely. They only make sense inside a loop body, and the closure form (`loop(({ recur, done }) => ...)`) makes the scoping explicit.

#### Object operations → namespace?

`extractField`, `pick`, `merge`, `augment` are all object data-shaping operations. Could live under an `Obj` or `Object` namespace:

```ts
// Before:
pipe(extractField("name"), pick("first", "last"))

// After:
pipe(Obj.get("name"), Obj.pick("first", "last"))
```

**Probably not worth it.** These are used constantly and would add verbosity for no clarity. `extractField("name")` is already unambiguous. The postfix forms (`.get()`, `.pick()`, `.augment()`) are even clearer.

Exception: `merge()` is confusing as a top-level name. What does it merge? It takes a tuple of objects from `parallel()`. Could argue it belongs under a `Tuple` namespace or just stays as-is since it's rarely used directly (postfix `.augment()` is the common pattern).

#### Array operations → namespace?

`flatten`, `forEach`, `extractIndex` plus future `take`, `skip`, `enumerate`, `count`.

```ts
// Hypothetical:
pipe(forEach(action), Arr.flatten(), Arr.take(5))
```

**Not worth it for existing ones.** `flatten()` and `forEach()` are already clear. Future array utilities (`take`, `skip`, `enumerate`, `count`) could go either way — they're generic enough to be top-level, or specific enough to namespace. Decide when implementing them.

#### `dropResult` / `tap` / `withResource` → namespace?

These are composition patterns, not data transformations. They don't naturally group.

- `dropResult(action)` — could be `action.drop()` (postfix already exists via chaining: `action.then(drop())` or `action.drop()`)
- `tap(action)` — unique enough to stay top-level, or move to a `Side` / `Effect` namespace (ugly)
- `withResource(...)` — unique pattern, stay top-level

**Keep top-level.** These are one-offs with clear names.

#### `range` → namespace?

`range(0, 10)` produces a constant array. It's sugar over `constant([0, 1, ..., 9])`.

**Keep top-level** or remove entirely (it's trivially replaceable with `constant`).

## Proposed organization

### Top-level (stay as-is)

Core algebra — used in nearly every pipeline:

```ts
import { pipe, parallel, forEach, branch, loop, constant, identity, drop } from "@barnum/barnum";
```

These are the primitives everyone learns first. They should be the shortest imports.

### Top-level but could deprecate

```ts
import { tag, extractField, extractIndex, pick, merge, flatten } from "@barnum/barnum";
```

These are builtin handlers exposed directly. They work fine as top-level. `tag` is the generic escape hatch for ad-hoc unions. The rest are data-shaping utilities.

Rename `extractField` → `get` for consistency with postfix `.get()`. This is already proposed in POSTFIX_OPERATORS.md.

### Top-level composition patterns

```ts
import { augment, tap, dropResult, withResource, range } from "@barnum/barnum";
```

TypeScript-only combinators. Stay top-level — they don't naturally group.

### Namespaces (new)

```ts
import { Option, Result, LoopResult } from "@barnum/barnum";

Option.some()
Option.map(action)
Option.andThen(action)
Option.collect()

Result.ok()
Result.map(action)
Result.andThen(action)

LoopResult.recur()
LoopResult.done()
LoopResult.mapBreak(action)
```

Tagged union operations grouped by their type. Each namespace knows the full variant map.

### Deprecate (move to namespace)

| Current top-level | Move to | When |
|---|---|---|
| `recur()` | `LoopResult.recur()` | When LoopResult namespace ships |
| `done()` | `LoopResult.done()` | When LoopResult namespace ships |

These are the only ones that clearly belong in a namespace. Everything else is fine top-level.

## Postfix methods: namespace interaction

Postfix methods on TypedAction (`.branch()`, `.get()`, `.drop()`, etc.) are always available regardless of namespacing. They're the preferred API for chaining.

Option/Result/LoopResult-specific postfix methods (`.mapOption()`, `.unwrapOr()`, etc.) are gated by `this` constraints and only visible when `Out` matches the expected type.

The namespace functions (`Option.map(action)`) are the standalone / prefix form. Postfix methods are the chained form. Both produce identical AST.

```ts
// These are equivalent:
pipe(lookup, Option.map(normalize))
lookup.then(Option.map(normalize))

// And when postfix Option methods exist:
lookup.mapOption(normalize)
```

## Open questions

### Should `tag` get a namespace?

`tag` is the generic constructor. It requires `<TDef, TKind>` type params. Could it live on a `Union` namespace?

```ts
Union.tag<ClassifyResultDef, "HasErrors">("HasErrors")
```

Probably not — `tag` is clear on its own, and a `Union` namespace just for `tag` is silly.

### What about `chain`?

`chain(a, b)` is binary `pipe`. It exists for internal use and edge cases. Should it stay exported? It's rarely used by consumers — `pipe` covers every use case.

**Probably remove from public API.** Keep as internal utility. But no rush — it doesn't hurt anyone.

### Naming: `LoopResult` is verbose

`LoopResult.recur()` is a lot of characters. Alternatives:

- `Loop.recur()` — shorter, but collides with the `loop()` combinator
- `LR.recur()` — too cryptic
- Just use the closure form: `loop(({ recur, done }) => ...)` — scoped, no namespace needed

The closure form is the best answer. `LoopResult` namespace is for when you need a standalone combinator (e.g. `LoopResult.mapBreak(action)` in a pipe).
