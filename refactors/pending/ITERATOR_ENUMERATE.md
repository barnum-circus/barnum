# Iterator.enumerate

**Status:** Design. Not approved. No implementation started.

## Goal

Add `Iterator.enumerate` matching Rust's `Iterator::enumerate`:

```
Iterator<T> → Iterator<[number, T]>
```

Tuple order is `[index, element]`, exactly as Rust yields `(usize, T)`. Indices
start at `0`.

## Constraint

`enumerate` is **not** a primitive. It must compose from a lower-level builtin
unless composition is impossible. The only capability we lack is producing an
index alongside each element; everything else (`map`, `collect`, `fromArray`,
`fold`/`loop`) already exists.

## Future direction: lazy iterators

Iterators will become lazy. This is the dominant design force here, so the
primitive we add must serve the lazy model, not just today's eager
`Array`-backed one.

Under laziness, `enumerate` is a stateful per-element step: carry a counter,
emit `[counter, element]`, advance the counter by one. There is no
materialized accumulator and no array growth. The primitive that serves this is
**integer increment** plus the existing per-element machinery — not a bespoke
`Enumerate` node that bakes in array materialization.

This kills the `Enumerate`-builtin option outright: a single `Enumerate` node
that maps `Array<T> → Array<[number, T]>` is an eager-only construct. It would
have to be rewritten when iterators go lazy. We do not add a primitive we know
we will delete.

## Current state

### Engine threads the index, but does not expose it

`crates/barnum_engine/src/advance.rs:100-131` — `ForEach` dispatches `body` to
each element with the element's position `i` already in hand:

```rust
for (i, element) in elements.into_iter().enumerate() {
    advance(
        workflow_state,
        body,
        element,
        Some(ParentRef::ForEach { frame_id, child_index: i }),
    )?;
}
```

The index `i` exists in the engine at dispatch time. It is used only for result
placement (`child_index`), never passed into `body`. So the body of a
`forEach`/`Iterator.map` cannot see its own index today.

### No arithmetic, append, zip, or length builtin

`crates/barnum_ast/src/lib.rs:238-320` defines `BuiltinKind`. The full set:
`Constant`, `Identity`, `Drop`, `Merge`, `Flatten`, `GetField`, `GetIndex`,
`CollectSome`, `SplitFirst`, `SplitLast`, `WrapInField`, `Sleep`,
`ExtractPrefix`, `AsOption`, `Panic`, `Slice`.

There is no `Add`/`Increment`, no array append/cons, no `Zip`, no `Length`. So
`fold`-with-a-counter cannot be expressed today: the body needs `idx + 1`, and
nothing produces it.

### fold accumulator cost

`libs/barnum/src/iterator.ts:173-215` implements `fold` over a `loop` that
threads `[acc, remaining]` as state. Each step does `all(newAcc, tail)`, and the
engine `value.clone()`s the loop state on each iteration
(`crates/barnum_engine/src/advance.rs:163,188`). Threading state through the
loop therefore copies the carried state per step. This matters for the
implementation choice below.

## Decision: expose the engine's index via a new `ForEachIndexed` action

No new builtin, no arithmetic. The engine already computes the index at
`advance.rs:120`. We expose it to the body of a *new* action variant so that
`enumerate` is an O(n), lazy-compatible map, and `map`/`filter`/`flatMap` keep
their current `element`-only contract untouched.

### Why a new action, not a change to `ForEach`

Plain `ForEach` feeds `element` straight to its body. `Iterator.map`, `filter`,
and `flatMap` all rely on that contract (`iterator.ts:99-137`). Changing
`ForEach` to feed `[index, element]` would force every existing body to
destructure and would widen the common case to serve the rare one. A separate
`ForEachIndexed` whose body receives `[index, element]` keeps signatures narrow:
the only caller is `enumerate`.

### Rust changes

1. **`crates/barnum_ast/src/lib.rs`**
   - Add `Action::ForEachIndexed(ForEachIndexedAction)` beside `ForEach`
     (line 81).
   - Add the payload struct mirroring `ForEachAction` (line 130-135):
     ```rust
     /// Parallel map over an array input, exposing each element's index.
     /// The body receives `[index, element]`.
     #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
     pub struct ForEachIndexedAction {
         /// The action to apply to each `[index, element]` pair.
         pub action: Box<Action>,
     }
     ```

2. **`crates/barnum_ast/src/flat.rs`**
   - Add `FlatAction::ForEachIndexed { body: ActionId }` beside `ForEach`
     (line 97-101) — same single-`body` shape.
   - In the flattener (the `Action::ForEach` arm near line 426), add a parallel
     `Action::ForEachIndexed` arm that flattens its body identically and emits
     `FlatAction::ForEachIndexed { body: body_id }`.

3. **`crates/barnum_engine/src/advance.rs`**
   - Add a `FlatAction::ForEachIndexed { body }` arm that is identical to the
     `ForEach` arm (line 100-131) except the per-element dispatch injects the
     index:
     ```rust
     for (i, element) in elements.into_iter().enumerate() {
         let indexed = Value::Array(vec![Value::from(i), element]);
         advance(
             workflow_state,
             body,
             indexed,
             Some(ParentRef::ParentRef::ForEach { frame_id, child_index: i }),
         )?;
     }
     ```
     Frame creation, `FrameKind::ForEach { results }`, and `ParentRef::ForEach`
     are reused verbatim — result placement is identical; only the body input
     shape differs. The empty-array vacuous-completion branch (line 107-113) is
     copied as-is.

   No new `FrameKind` or `ParentRef` variant is needed: indexing changes only
   what the body *receives*, not how its result is collected.

### TS changes

4. **`libs/barnum/src/ast.ts`** — add a `forEachIndexed(action)` constructor
   beside `forEach` (referenced at `iterator.ts:8`), producing the
   `ForEachIndexed` action. The body's input type is `[number, TElement]`.

5. **`libs/barnum/src/iterator.ts`** — add `Iterator.enumerate`:
   ```ts
   /** Pair each element with its zero-based index. `Iterator<T> → Iterator<[number, T]>` */
   enumerate<TElement>(): TypedAction<
     IteratorT<TElement>,
     IteratorT<[number, TElement]>
   > {
     return Iterator.collect<TElement>()
       .then(forEachIndexed(identity<[number, TElement]>()))
       .then(Iterator.fromArray<[number, TElement]>());
   }
   ```
   The body is `identity()` because `ForEachIndexed` already delivers
   `[index, element]`; `enumerate` just passes the pair through. Tuple order is
   `[index, element]`, matching Rust.

### Cost

O(n). The index is produced by the engine's existing loop counter; there is no
accumulator, no append, no arithmetic builtin, and no per-step state clone. This
is exactly the shape a lazy `enumerate` will use (per-element counter step), so
nothing here is throwaway when iterators go lazy.

## Open questions

1. **`Value::from(i)`** — confirm `serde_json::Value` is constructed from a
   `usize` index the way the rest of the engine builds numeric values (the
   existing builtins use `json!(...)`; match whatever convention
   `barnum_builtins` uses for numbers).

2. **Generalizes to indexed `map`/`filter`?** `ForEachIndexed` could back a
   future `Iterator.mapIndexed` / `filterIndexed`. Out of scope here — mention
   only, do not implement (per "do exactly what is asked").

## Test-first plan (per PROCESS.md)

1. Rust: `#[should_panic]` engine test asserting a `ForEachIndexed` over
   `["a","b","c"]` dispatches bodies receiving `[0,"a"]`, `[1,"b"]`, `[2,"c"]`.
2. TS: `iterator.test.ts` test asserting
   `[10,20,30].iterate().enumerate().collect()` yields
   `[[0,10],[1,20],[2,30]]`, and that empty input yields `[]`.
3. Implement, then drop the failure markers.
