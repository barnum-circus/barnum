# Pipe → Then

Replace the N-ary `Pipe` AST node with binary `Then` across the entire stack.

## Motivation

`Pipe` is the only combinator where child completion triggers lateral movement (advance the next child) instead of upward propagation. It requires mutable state (an index) in its frame, making it unlike every other combinator. Replacing `Pipe([A, B, C])` with right-nested `Then(A, Then(B, C))` makes the engine uniform: every frame is either single-child (completes or re-enters on child completion) or fan-out (fills slots, collects when full). No special cases, no mutation.

## The transformation

```
Pipe([A])          →  A
Pipe([A, B])       →  Then(A, B)
Pipe([A, B, C])    →  Then(A, Then(B, C))
Pipe([A, B, C, D]) →  Then(A, Then(B, Then(C, D)))
```

Right-associative nesting. The `pipe()` TypeScript combinator still exists as a user-facing convenience — it produces nested `Then` nodes. No change to user API.

## Changes

### 1. TypeScript AST (`libs/barnum/src/ast.ts`)

Remove `PipeAction`. Add `ThenAction`.

```ts
// Remove:
export type PipeAction = {
  kind: "Pipe";
  actions: Action[];
};

// Add:
export type ThenAction = {
  kind: "Then";
  first: Action;
  rest: Action;
};
```

Update the `Action` union type accordingly.

### 2. TypeScript combinator (`libs/barnum/src/ast.ts`)

The `pipe()` function becomes a right-fold that produces nested `Then` nodes:

```ts
export function pipe(...actions: Action[]): Action {
  if (actions.length === 0) throw new Error("pipe requires at least one action");
  if (actions.length === 1) return actions[0];
  return actions.reduceRight((rest, first) => ({ kind: "Then", first, rest }));
}
```

Callers don't change. The only difference is the serialized JSON shape.

### 3. Rust tree AST (`crates/barnum_ast/src/lib.rs`)

Remove `Action::Pipe(PipeAction)`. Add `Action::Then(ThenAction)`.

```rust
// Remove:
pub struct PipeAction {
    pub actions: Vec<Action>,
}

// Add:
pub struct ThenAction {
    pub first: Box<Action>,
    pub rest: Box<Action>,
}
```

Serde tag is `"Then"`. Both fields deserialized directly from JSON.

### 4. Flat action table (`crates/barnum_ast/src/flat.rs`)

Remove `FlatAction::Pipe { count: Count }`. Add `FlatAction::Then { first: ActionId, rest: ActionId }`.

```rust
// Remove:
Pipe { count: Count },

// Add:
Then { first: ActionId, rest: ActionId },
```

`Then` has two `ActionId` fields (two u32s). This increases `FlatEntry<ActionId>` from 8 bytes to 12 bytes. The 8-byte property was nice but not load-bearing — correctness and uniformity matter more.

**Alternative (preserve 8 bytes):** Store only `rest: ActionId` in the variant, and convention that `first` is the child slot at `action_id + 1` (like current Pipe child slots). This makes Then a 2-entry action (Then + child slot), reusing the existing child slot machinery. Tradeoff: preserves size at the cost of more indirection. Probably not worth it given that Pipe's child slot machinery is the thing we're eliminating.

### 5. Flattening (`crates/barnum_ast/src/flat.rs`)

Remove the `Pipe` arm in `flatten_action_at`. Add `Then`:

```rust
Action::Then(ThenAction { first, rest }) => {
    let first_id = self.flatten_action(*first, workflow_root)?;
    let rest_id = self.flatten_action(*rest, workflow_root)?;
    FlatAction::Then { first: first_id, rest: rest_id }
}
```

Both children are flattened via `flatten_action` (allocated elsewhere, referenced by ActionId). No child slots, no `alloc_many`, no `fill_child_slots` for Then.

This simplifies flattening: only `Parallel` and `Branch` use the multi-entry child slot model. The `fill_child_slot` inlining optimization (single-entry children inlined, multi-entry via ChildRef) still applies to those two.

### 6. FlatConfig accessors

Remove `children()` (was Pipe/Parallel). Split into:
- `parallel_children()` — for Parallel only
- `branch_cases()` — unchanged

Or keep `children()` for Parallel only and rename to `parallel_children()` for clarity.

### 7. Engine (not yet implemented, but ENGINE.md updated)

Already done. `Then { rest }` frame replaces `Pipe { action_id, index }`.

### 8. Flatten tests

Update all Pipe tests to use Then. The basic structure tests change shape:

```rust
// Old: Pipe([A, B, C]) → [Pipe{3}, Invoke(0), Invoke(1), Invoke(2)]
// New: pipe(A, B, C) → Then(A, Then(B, C))
//   → [Then{first:1, rest:2}, Invoke(0), Then{first:3, rest:4}, Invoke(1), Invoke(2)]
```

Nested Pipe tests simplify — no more ChildRef for inner Pipes, since Then is always 1 entry (no child slots).

### 9. JSON schema regeneration

After changing the AST types, regenerate:
- `libs/barnum/barnum-config-schema.json`
- `libs/barnum/barnum-config-schema.zod.ts`
- `libs/barnum/barnum-cli-schema.zod.ts`

### 10. TypeScript tests

Update any tests that assert on the serialized Pipe shape. The `pipe()` combinator tests should verify the nested Then output.

## What simplifies

- **Flattening**: Pipe was one of three multi-entry action types (with Parallel and Branch). Then is single-entry. `fill_child_slots` is only needed for Parallel. Less code, fewer edge cases.
- **Engine**: No index tracking, no frame mutation for sequential execution. Every frame follows the single-child or fan-out pattern.
- **`children()` accessor**: Was shared by Pipe and Parallel. With Pipe gone, it's just Parallel's accessor.
- **ChildRef entries**: Fewer of them. Pipe's children could be ChildRefs (for nested multi-entry children). Then's children are always ActionIds — no indirection.

## What doesn't change

- **User-facing `pipe()` API**: Still exists, still takes variadic actions. Just produces different AST nodes.
- **Parallel, Branch, ForEach, Loop, Attempt, Step, Invoke**: All unchanged.
- **Two-pass step resolution**: Unchanged (Step is orthogonal to this).
- **Handler interning**: Unchanged.

## Size impact

`FlatEntry<ActionId>` grows from 8 to 12 bytes. For a workflow with N sequential steps, the old representation used `1 + N` entries at 8 bytes each (`Pipe + N child slots`). The new representation uses `N - 1 + N = 2N - 1` entries at 12 bytes each (`N-1 Then nodes + N leaf actions`). For N=5: old = 48 bytes, new = 108 bytes. The absolute numbers are tiny (workflow configs are small), but the constant-factor increase is real.

If this matters later, the 8-byte alternative (Then stores only `rest`, `first` in a child slot) recovers most of it.

## Implementation order

1. TypeScript: add `ThenAction` type, update `pipe()` to produce nested Then, update tests
2. Rust tree AST: replace `PipeAction` with `ThenAction`, update serde
3. Rust flat AST: replace `FlatAction::Pipe` with `FlatAction::Then`, update flattening
4. Update flatten tests
5. Regenerate schemas
6. Verify CI
