# Branching

`branch` routes a tagged union to different handlers based on its `kind` discriminant. Each case handler receives the unwrapped payload — not the full `{ kind, value }` wrapper.

## Basic branching

From [`demos/convert-folder-to-ts/handlers/type-check-fix.ts`](https://github.com/barnum-circus/barnum/tree/master/demos/convert-folder-to-ts/handlers/type-check-fix.ts):

```ts
export const typeCheckFix = loop((recur) =>
  typeCheck.then(classifyErrors).branch({
    HasErrors: Iterator.fromArray<TypeError>().map(fix).drop().then(recur),
    Clean: drop,
  }),
);
```

`classifyErrors` returns a tagged union — either `{ kind: "HasErrors", value: TypeError[] }` or `{ kind: "Clean", value: void }`. The `HasErrors` handler receives the `TypeError[]` of errors directly; the `Clean` handler receives `void`.

## Branching with Result types

Result combinators like `.unwrapOr()` are built on `branch` internally. You can also branch on Results explicitly:

```ts
riskyStep.branch({
  Ok: processSuccess,
  Err: handleFailure,
})
```

## Branching with Option types

The same pattern works for `Option<T>`:

```ts
maybeFind.branch({
  Some: processFound,
  None: constant("not found"),
})
```
