# Pipe → Chain

Replace the N-ary `Pipe` AST node with binary `Chain` across the entire stack.

## Motivation

`Pipe` is the only combinator where child completion triggers lateral movement (advance the next child) instead of upward propagation. It requires mutable state (an index) in its frame, making it unlike every other combinator. Replacing `Pipe([A, B, C])` with right-nested `Chain(A, Chain(B, C))` makes the engine uniform: every frame is either single-child (completes or re-enters on child completion) or fan-out (fills slots, collects when full). No special cases, no mutation.

## The transformation

```
pipe()             →  identity()       (built-in, neutral element)
pipe(A)            →  A
pipe(A, B)         →  Chain(A, B)
pipe(A, B, C)      →  Chain(A, Chain(B, C))
pipe(A, B, C, D)   →  Chain(A, Chain(B, Chain(C, D)))
```

Right-associative nesting. The `pipe()` TypeScript combinator still exists as a user-facing convenience with all its type-level overloads unchanged — only the runtime implementation body changes to produce nested `Chain` nodes.

## Changes

### 1. TypeScript AST (`libs/barnum/src/ast.ts`)

Remove `PipeAction`. Add `ChainAction`.

```ts
// Remove:
export type PipeAction = {
  kind: "Pipe";
  actions: Action[];
};

// Add:
export type ChainAction = {
  kind: "Chain";
  first: Action;
  rest: Action;
};
```

Update the `Action` union type accordingly.

### 2. TypeScript combinator (`libs/barnum/src/pipe.ts`)

All type-level overloads stay identical. Only the implementation body changes:

```ts
// Before:
export function pipe(...actions: Action[]): Action {
  return { kind: "Pipe", actions };
}

// After:
export function pipe(...actions: Action[]): Action {
  if (actions.length === 0) return identity();
  if (actions.length === 1) return actions[0];
  return actions.reduceRight(
    (rest, first) => ({ kind: "Chain", first, rest } as Action),
  );
}
```

`pipe()` with zero actions returns `identity()` — the built-in neutral element of sequential composition.

### 3. Rust tree AST (`crates/barnum_ast/src/lib.rs`)

Remove `Action::Pipe(PipeAction)`. Add `Action::Chain(ChainAction)`.

```rust
// Remove:
pub struct PipeAction {
    pub actions: Vec<Action>,
}

// Add:
pub struct ChainAction {
    pub first: Box<Action>,
    pub rest: Box<Action>,
}
```

Serde tag is `"Chain"`. Both fields deserialized directly from JSON.

### 4. Flat action table (`crates/barnum_ast/src/flat.rs`)

Remove `FlatAction::Pipe { count: Count }`. Add `FlatAction::Chain { rest: ActionId }`.

```rust
// Remove:
Pipe { count: Count },

// Add:
/// Sequential: run child at action_id + 1, then advance to rest.
Chain { rest: ActionId },
```

Chain is a **2-entry action**: the Chain entry itself (with `rest` as an explicit `ActionId` field), followed by one child slot for `first`. The child slot is either an inlined single-entry action or a `ChildRef` to a multi-entry subtree elsewhere.

This is optimal for the common case: `Chain(Invoke, Chain(...))`. The Invoke (single-entry) inlines into the child slot — zero indirection. The `rest` (often another Chain) is a direct `ActionId` — also zero indirection. `ChildRef` is only needed when `first` is multi-entry (Parallel, Branch, or another Chain), which is uncommon.

`FlatEntry<ActionId>` stays 8 bytes. Chain has one `ActionId` field, same as ForEach/Loop/Attempt.

### 5. Flattening (`crates/barnum_ast/src/flat.rs`)

Remove the `Pipe` arm in `flatten_action_at`. Add `Chain`:

```rust
Action::Chain(ChainAction { first, rest }) => {
    self.alloc();  // child slot for first
    let action_id = self.flatten_action(*rest, workflow_root)?;
    self.fill_child_slot(*first, chain_action_id + 1, workflow_root)?;
    FlatAction::Chain { rest: action_id }
}
```

`rest` is flattened via `flatten_action` (allocated elsewhere, referenced by `ActionId`). `first` uses `fill_child_slot` — inlined if single-entry, `ChildRef` if multi-entry.

The `fill_child_slot` multi-entry check adds `Chain` to the list:

```rust
// Before:
Action::Pipe { .. } | Action::Parallel { .. } | Action::Branch { .. } => { ... ChildRef ... }

// After:
Action::Chain { .. } | Action::Parallel { .. } | Action::Branch { .. } => { ... ChildRef ... }
```

### 6. FlatConfig accessors

Rename `children()` to `parallel_children()` — it's now only used by Parallel.

Add a Chain accessor:

```rust
/// Returns the first child `ActionId` for a Chain (resolves the child slot at action_id + 1).
/// The rest `ActionId` is stored in the Chain variant itself.
fn chain_first(&self, id: ActionId) -> ActionId {
    debug_assert!(matches!(self.action(id), FlatAction::Chain { .. }));
    self.resolve_child_slot(id + 1)
}
```

### 7. Engine (not yet implemented, but ENGINE.md updated)

Already done. `Chain { rest }` frame replaces `Pipe { action_id, index }`. The engine resolves `first` from the flat table during `advance`, stores only `rest` in the frame (since `first` is being advanced immediately).

### 8. Flatten tests

Update all Pipe tests to use Chain. Example layout change:

```rust
// Old: Pipe([A, B, C]) → [Pipe{3}, Invoke(0), Invoke(1), Invoke(2)]
//   4 entries, all children inlined

// New: pipe(A, B, C) → Chain(A, Chain(B, C))
//   DFS allocation:
//   0: Chain { rest: 2 }    ← outer Chain, rest points to inner Chain
//   1: Invoke(handler_0)    ← child slot (inlined: A is single-entry)
//   2: Chain { rest: 4 }    ← inner Chain (allocated by flatten_action for rest)
//   3: Invoke(handler_1)    ← child slot (inlined: B)
//   4: Invoke(handler_2)    ← inner rest (allocated by flatten_action: C)
//   5 entries total. No ChildRefs — all firsts are single-entry Invokes.
```

When `first` is multi-entry (e.g. `pipe(parallel(...), invoke(...))`), the child slot at `action_id + 1` contains a `ChildRef` pointing to the Parallel allocated elsewhere. `rest` is unaffected — it's always a direct `ActionId` field, not a child slot.

### 9. JSON schema regeneration

After changing the AST types, regenerate:
- `libs/barnum/barnum-config-schema.json`
- `libs/barnum/barnum-config-schema.zod.ts`
- `libs/barnum/barnum-cli-schema.zod.ts`

### 10. TypeScript tests

Update any tests that assert on the serialized Pipe shape. The `pipe()` combinator tests should verify the nested Chain output.

## What simplifies

- **Engine**: No index tracking, no frame mutation for sequential execution. Every frame follows the single-child or fan-out pattern. `ParentRef` becomes an enum (SingleChild vs IndexedChild) — impossible states unrepresentable.
- **`children()`/`fill_child_slots()`**: No longer shared by Pipe and Parallel. Chain uses `fill_child_slot` (singular) once (for `first`). `rest` is a direct `ActionId`. Parallel keeps `fill_child_slots` (plural) for N children. Clearer separation.

## What doesn't change

- **User-facing `pipe()` API**: All overloads identical. Just the implementation body changes.
- **Parallel, Branch, ForEach, Loop, Attempt, Step, Invoke**: All unchanged.
- **Child slot model**: Chain uses the same inlining/ChildRef machinery as Parallel and Branch.
- **Two-pass step resolution**: Unchanged.
- **Handler interning**: Unchanged.
- **8-byte `FlatEntry<ActionId>`**: Preserved. Chain has one `ActionId` field.

## Size impact

For a pipeline of N steps, old representation: `1 + N` entries (Pipe + N child slots). New representation: `2(N-1) + 1` entries in the typical case (`N-1` Chain entries each with 1 child slot, plus the final leaf). For `pipe(A, B, C)`: old = 4 entries (32 bytes), new = 5 entries (40 bytes). Entry size is unchanged at 8 bytes each. The entry count increases slightly but absolute sizes remain tiny for workflow configs.

## Implementation order

1. TypeScript: add `ChainAction` type, update `pipe()` body to produce nested Chain, update tests
2. Rust tree AST: replace `PipeAction` with `ChainAction`, update serde
3. Rust flat AST: replace `FlatAction::Pipe` with `FlatAction::Chain`, update flattening and `fill_child_slot` check
4. Update flatten tests
5. Regenerate schemas
6. Verify CI
