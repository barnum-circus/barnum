# `.call()` on TypedAction

## Motivation

Inside `bindInput` bodies with `.split()`, assembling arguments for an action requires ceremony:

```ts
// From implementationConsumer:
bindInput<ImplState, string>((state) => {
  const { pendingRefactors, lastBranch } = state.split();
  return pendingRefactors.iterate().fold(lastBranch, processSingleRefactor(dir, file));
});
```

This is fine — the fold body receives `[string, PotentialRefactor]` as its input. But when you need to invoke an action that accepts an object/tuple built from multiple VarRefs, you write:

```ts
allObject({ file, refactor, worktreePath }), triageRefactor(dir)
// or
allObject({ file, refactorName: refactor.getField("refactorName"), baseBranch }), createImplBranch
```

The pattern is always: assemble VarRefs into a structure, then call the action. `.call()` makes the intent explicit: "invoke this action with these arguments."

---

## API: `.call(input)`

**Single method. One argument. That argument is `all(...)` or `allObject({...})`.**

```ts
call<TIn>(this: TypedAction<TIn, TOut>, input: Pipeable<any, TIn>): TypedAction<any, TOut>;
```

**Semantics:** `action.call(input)` ≡ `input.then(action)` ≡ `pipe(input, action)`.

That's it. It's `.then()` with subject and object swapped. No variadic arguments — you pass a single value. If that value needs to be assembled from multiple VarRefs, use `all()` or `allObject()`:

```ts
// Single VarRef:
getScore.call(item)                    // ≡ item.then(getScore)

// Multiple VarRefs (tuple):
processItem.call(all(acc, item))       // ≡ all(acc, item).then(processItem)

// Multiple VarRefs (object):
triageRefactor(dir).call(allObject({ file, refactor, worktreePath }))
// ≡ pipe(allObject({ file, refactor, worktreePath }), triageRefactor(dir))
```

---

## Implementation

### Type signature (in `TypedAction`)

```ts
export type TypedAction<In = unknown, Out = unknown> = Action & {
  // ... existing fields ...
  /** Invoke this action with a given input. `action.call(input)` ≡ `input.then(action)`. */
  call(input: Pipeable<any, In>): TypedAction<any, Out>;
};
```

The input type of the `.call()` result is `any` because the input to the composed action is whatever the `input` pipeable accepts — which varies. The caller never chains further from the input side of a `.call()` result; it's always used in a position where the pipeline input is irrelevant (inside `bindInput` where VarRefs already have `any` as their input type).

### Runtime implementation (in `typedAction`)

```ts
Object.defineProperty(obj, "call", {
  value(input: Pipeable<any, any>) {
    return typedAction(toAction(chain(toAction(input), toAction(obj))));
  },
  enumerable: false,
  configurable: true,
});
```

Identical to how `.then()` is implemented, just with operands swapped: `chain(input, this)` instead of `chain(this, next)`.

### AST

No new AST nodes. `action.call(input)` compiles to `Chain(input, action)` — the same node that `.then()` produces, with arguments flipped.

---

## Before/After from `process.ts`

### Example 1: `processSingleRefactor`

**Before:**
```ts
return pipe(allObject({ file, refactor, worktreePath }), triageRefactor(dir)).branch({
  Keep: pipe(
    allObject({
      file,
      refactorName: refactor.getField("refactorName"),
      baseBranch,
    }),
    createImplBranch,
    implementAndFix(dir, file, refactor, worktreePath, baseBranch),
  ),
  Skip: pipe(
    allObject({ file, refactor }),
    writeRefactorStatus({ dir, status: "skipped" }),
    baseBranch,
  ),
});
```

**After:**
```ts
return triageRefactor(dir).call(allObject({ file, refactor, worktreePath })).branch({
  Keep: pipe(
    createImplBranch.call(allObject({
      file,
      refactorName: refactor.getField("refactorName"),
      baseBranch,
    })),
    implementAndFix(dir, file, refactor, worktreePath, baseBranch),
  ),
  Skip: pipe(
    writeRefactorStatus({ dir, status: "skipped" }).call(allObject({ file, refactor })),
    baseBranch,
  ),
});
```

### Example 2: `onChecksPass`

**Before:**
```ts
return pipe(
  allObject({ file, refactor, worktreePath, baseBranch }),
  commitAndPr(dir),
  bindInput<string, never>((newBranch) => {
    return pipe(
      allObject({ file, refactor }),
      writeRefactorStatus({ dir, status: "done" }),
      allObject({ file, branchName: newBranch }),
      writeLastBranch(dir),
      newBranch,
      fixDone,
    );
  }),
);
```

**After:**
```ts
return pipe(
  commitAndPr(dir).call(allObject({ file, refactor, worktreePath, baseBranch })),
  bindInput<string, never>((newBranch) => {
    return pipe(
      writeRefactorStatus({ dir, status: "done" }).call(allObject({ file, refactor })),
      writeLastBranch(dir).call(allObject({ file, branchName: newBranch })),
      newBranch,
      fixDone,
    );
  }),
);
```

### Example 3: `onChecksFail`

**Before:**
```ts
return pipe(
  allObject({ file, refactor, feedback, worktreePath }),
  fixFromFeedback(dir),
  nextAttempt,
  fixRecur,
);
```

**After:**
```ts
return pipe(
  fixFromFeedback(dir).call(allObject({ file, refactor, feedback, worktreePath })),
  nextAttempt,
  fixRecur,
);
```

---

## When `.call()` helps vs. doesn't

**Helps:** When an action's input must be assembled from VarRefs. The action name reads first (verb), then the arguments (nouns). `triageRefactor(dir).call(allObject({...}))` reads as "triage refactor with file, refactor, worktreePath."

**Doesn't help:** Linear pipelines where data flows naturally via `.then()`. If the previous step's output is the next step's input, `.then()` is already perfect.

**Equivalence:**
- `action.call(ref)` ≡ `ref.then(action)` ≡ `pipe(ref, action)`
- `action.call(all(a, b))` ≡ `all(a, b).then(action)` ≡ `pipe(all(a, b), action)`
- `action.call(allObject({...}))` ≡ `pipe(allObject({...}), action)`

The user picks whichever reads best in context. `.call()` reads best when the action is the focus. `.then()` reads best when the data flow is the focus.

---

## Naming

**`.call()`** — chosen because:
1. It's the standard JS name for "invoke a function with explicit arguments"
2. TypedAction is not a Function — no collision at the value level
3. It reads naturally: `triageRefactor.call(args)` = "call triageRefactor with args"

`Function.prototype.call` passes `this` as the first arg — irrelevant here since TypedAction isn't callable. No user confusion expected.

---

## What we explicitly skip

**No `.bind()` (partial application).** TypeScript can't do variadic tuple subtraction. You'd need explicit type params which defeats ergonomics.

**No `.apply()`.** Historically `.apply()` takes an array. Our `.call()` already takes a single argument that can be `all(...)` — there's no separate "spread" case.

**No variadic `.call(a, b, c)`.** We don't need it. Nothing in the framework takes raw positional arguments that aren't already `all(...)` or `allObject({...})`. The one-argument form keeps the implementation trivial and the types simple.

---

## Summary of changes

| File | Change |
|------|--------|
| `libs/barnum/src/ast.ts` | Add `call` to `TypedAction` type definition |
| `libs/barnum/src/ast.ts` | Add `call` to `typedAction()` method attachment (≈3 lines) |
| `libs/barnum/src/index.ts` | No change needed — `call` is a method, not an export |

Total implementation: ~5 lines of runtime code, ~1 line of type definition.
