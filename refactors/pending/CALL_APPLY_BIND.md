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

## Nested `.call()` chains

Inside `bindInput` bodies where multiple actions need assembling from VarRefs, you can nest `.call()` calls. This is the `.call()` analog of deeply nested `.then()` — potentially an anti-pattern if overused, but shows the expressiveness:

```ts
// Nested calls — each action invoked with its assembled args
bindInput<ImplState, string>((state) => {
  const { pendingRefactors, lastBranch } = state.split();
  return pendingRefactors
    .iterate()
    .fold(
      lastBranch,
      bindInput<[string, PotentialRefactor], string>((pair) => {
        const [baseBranch, refactor] = pair.split();
        return triageRefactor(dir)
          .call(allObject({ file, refactor, worktreePath: resetWorktreeToBase.call(baseBranch) }))
          .branch({
            Keep: createImplBranch
              .call(allObject({ file, refactorName: refactor.getField("refactorName"), baseBranch }))
              .then(implementAndFix(dir, file, refactor, worktreePath, baseBranch)),
            Skip: writeRefactorStatus({ dir, status: "skipped" })
              .call(allObject({ file, refactor }))
              .then(baseBranch),
          });
      }),
    );
});
```

This reads as imperative "call X with Y, then call Z with W" — each line is a function invocation. Compare with the pipe-based version which reads as "assemble data, pass to function, assemble more data, pass to next function."

The tradeoff: deeply nested `.call()` can become hard to parse vertically, just like deeply nested method chains. In practice, extract helper functions (as `process.ts` already does) rather than nesting 5 levels deep.

---

## Mental model: `pipe`, `.then()`, and `.call()`

**`pipe(a, b, c)` = sequential let bindings:**
```ts
pipe(a, b, c)
// Mental model:
// let v1 = a(input)
// let v2 = b(v1)
// let v3 = c(v2)
// return v3
```

Each step receives the previous step's output. The "current value" flows left-to-right through the pipeline. This is the default for linear data transformation — no assembly needed, each step consumes what the last produced.

**`.then()` = single-step continuation:**
```ts
ref.then(action)
// Mental model:
// let v = ref()
// return action(v)
```

Same as pipe but for a single link. Reads as "take this value, feed it to action." Best when the subject (the data source) is the focus.

**`.call()` = function invocation:**
```ts
action.call(input)
// Mental model:
// return action(input())
```

Same semantics as `.then()` but with subject/object swapped. Reads as "invoke action with input." Best when the verb (the action) is the focus — especially when the input is assembled from multiple VarRefs.

### When to use which

| Pattern | Use when... |
|---------|------------|
| `pipe(a, b, c)` | Linear: each step consumes the previous step's output |
| `ref.then(action)` | One-step: you have a value and want to transform it |
| `action.call(input)` | Invocation: you have an action and want to supply its argument |

In `bindInput` bodies, `.call()` is almost always clearer than `pipe(allObject({...}), action)` because the action is the important thing — the assembly is just plumbing.

### Does `.call()` replace `pipe`?

No. `pipe` composes a linear sequence where data flows step-to-step. A "reversed pipe" would be `action3.call(action2.call(action1.call(input)))` — nesting from inside out, losing the left-to-right readability that `pipe` provides. That's strictly worse.

`.call()` replaces the pattern `pipe(assembled_input, action)` (two-step pipes where the first step is assembly). It doesn't replace `pipe(a, b, c, d)` (multi-step linear flows).

### Does `.call()` replace `.then()`?

Partially. `ref.then(action)` and `action.call(ref)` are equivalent. The choice is stylistic:
- `file.then(readContents)` — "take file, read its contents" (data-focused)
- `readContents.call(file)` — "read contents of file" (verb-focused)

In practice, `.call()` dominates inside `bindInput` bodies because you're thinking imperatively: "I have VarRefs, I want to invoke functions on them." `.then()` dominates in method chains: `iterator.splitFirst().branch({...})`.

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

## Implementation changes

| File | Change |
|------|--------|
| `libs/barnum/src/ast.ts` | Add `call` to `TypedAction` type definition |
| `libs/barnum/src/ast.ts` | Add `call` to `typedAction()` method attachment (≈3 lines) |

Total implementation: ~5 lines of runtime code, ~1 line of type definition.

---

## Files to rewrite after implementation

Mechanical rewrite: replace `pipe(allObject({...}), action)` / `allObject({...}).then(action)` / `pipe(all(...), action)` with `action.call(...)` where it reads better. Also update docs to teach `.call()` as the default pattern in `bindInput` bodies.

### Demos

- `demos/sequential-deploy/run.ts`
- `demos/implement-feature/run.ts`
- `demos/implement-feature/handlers/with-retry.ts`
- `demos/implement-feature/handlers/with-max-attempts.ts`
- `demos/identify-and-address-refactors/run.ts`
- `demos/identify-and-address-refactors/handlers/refactor.ts`
- `demos/identify-and-address-refactors/handlers/type-check-fix.ts`
- `demos/convert-folder-to-ts/run.ts`
- `demos/convert-folder-to-ts/handlers/type-check-fix.ts`
- `demos/babysit-prs/run.ts`
- `demos/retry-on-error/run.ts`
- `demos/event-bus/run.ts`
- `demos/event-bus-durable/run.ts`
- `demos/workflow-output/run.ts`

### Tests

- `libs/barnum/tests/bind.test.ts`
- `libs/barnum/tests/struct.test.ts`
- `libs/barnum/tests/with-resource.test.ts`
- `libs/barnum/tests/iterator.test.ts`
- `libs/barnum/tests/loop.test.ts`

### Documentation

- `docs-website/docs/reference/builtins.md` — add `.call()` to TypedAction method reference
- `docs-website/docs/reference/best-practices.md` — rewrite `bindInput` section to use `.call()` as the default pattern; add mental model section (pipe = let bindings, call = function invocation)
- `docs-website/docs/patterns/bounded-concurrency.md` — rewrite with `.call()`
- `docs-website/docs/repertoire/sequential-file-processing.md` — rewrite with `.call()`
- `docs-website/docs/architecture/algebraic-effect-handlers.md` — update if examples use assembly pattern
