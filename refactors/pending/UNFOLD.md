# Unfold — loop without explicit recur/done routing

## Motivation

The most common `loop` pattern is:

```ts
loop<TResult, TState>((recur, done) =>
  body.branch({
    Continue: recur,
    Done: done,
  })
)
```

The `(recur, done) =>` callback and `.branch({ Continue: recur, Done: done })` are pure boilerplate. The actual information is: "run `body`, if it returns `Continue(state)` keep going, if it returns `Done(result)` exit." This is `unfold` from functional programming.

`loop` with explicit `recur`/`done` tokens remains valuable for complex cases (retry loops, adversarial review where the branch logic is non-trivial, multiple recur points). But for the simple "step function returns a signal" case, `unfold` eliminates the noise.

## Proposed API

```ts
function unfold<TDone, TState>(
  body: Pipeable<TState, LoopSignal<TState, TDone>>,
): TypedAction<TState, TDone>;
```

Where `LoopSignal` is a tagged union:

```ts
type LoopSignal<TState, TDone> =
  | { kind: "LoopSignal.Continue"; value: TState }
  | { kind: "LoopSignal.Done"; value: TDone };
```

### Usage

```ts
// Before:
loop<LoopResult, LoopState>((recur, done) =>
  pipe(processItem, advanceOrFinish).branch({
    Continue: recur,
    Done: done,
  })
)

// After:
unfold(pipe(processItem, advanceOrFinish))
```

The handler `advanceOrFinish` would return `LoopSignal` variants instead of custom ones:

```ts
export const advanceOrFinish = createHandler({
  inputValidator: loopStateSchema,
  outputValidator: LoopSignal.schema(loopStateSchema, z.array(z.number())),
  handle: async ({ value: state }) => {
    if (state.rest.length === 0) {
      return { kind: "LoopSignal.Done" as const, value: state.results };
    }
    return {
      kind: "LoopSignal.Continue" as const,
      value: { current: state.rest[0], rest: state.rest.slice(1), results: state.results },
    };
  },
}, "advanceOrFinish");
```

### Schema helper

```ts
// In the LoopSignal namespace:
LoopSignal.schema<TState, TDone>(
  stateSchema: z.ZodType<TState>,
  doneSchema: z.ZodType<TDone>,
): z.ZodType<LoopSignal<TState, TDone>>
```

Parallels `Option.schema()` and `Result.schema()`.

## Implementation

`unfold` composes from existing primitives — no new AST nodes:

```ts
export function unfold<TDone, TState>(
  body: Pipeable<TState, LoopSignal<TState, TDone>>,
): TypedAction<TState, TDone> {
  return loop<TDone, TState>((recur, done) =>
    typedAction<TState, never>(
      toAction(chain(toAction(body), toAction(branch({ Continue: recur, Done: done })))),
    ),
  );
}
```

The `branch` matches on the `LoopSignal` variants (after namespace stripping, `"LoopSignal.Continue"` becomes `"Continue"` at the branch level).

## When to use `loop` vs `unfold`

| Use `unfold` | Use `loop` |
|---|---|
| Body returns a two-variant "continue or done" signal | Multiple recur points (retry after error handling) |
| Single linear body with one decision point at the end | Branch logic is non-trivial (e.g., `NeedsWork: applyFeedback.then(recur)`) |
| The step function is a handler that naturally returns Continue/Done | `recur` appears in the middle of a chain, not at the end of a branch |

## Relationship to fold

`fold` (from `FOLD_AND_SPLITS.md`) threads an accumulator through iterator elements. `unfold` is the general case — it loops arbitrary state until a done signal. `fold` could be reimplemented in terms of `unfold` + `splitFirst`, though the existing implementation is fine.

## Open questions

1. **Variant names**: `Continue`/`Done` vs `Recur`/`Break` vs `Next`/`Done`. `Continue`/`Done` reads naturally in handler return statements and matches the user's existing code. `Recur`/`Break` matches the `loop` parameter names.
2. **Postfix**: should there be a `.unfold(body)` postfix on any `TypedAction` whose output is `TState`? Probably yes — `initState.then(unfold(step))` vs `initState.unfold(step)`.

-----

- seems worth doing, yes to postfix
- recur/done are only because the others are reserved words in JS, and I like the existing names
- is the naming good tho? foo.fold(...) makes sense, it's array -> item, item -> array is not this? I mean, I get how it is, but then the return value should be an array! (The return value should be TDone, that's fine)
- is this just post-fix loop?
- in some form, this should exist, perhaps it's just post-fix loop though? I don't necessarily see the benefit of unfold(pipeline) vs the explicit loop((recur, done) => ... )