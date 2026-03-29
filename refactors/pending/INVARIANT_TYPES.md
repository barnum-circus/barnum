# Invariant Types

## Motivation

Barnum's handler boundary is a serialization boundary. Data gets JSON-encoded and sent to a subprocess (TypeScript, Rust, eventually other languages). Allowing extra fields to pass through this boundary is:

1. **An information leak**: the handler sees data it didn't declare in its schema.
2. **Language-dependent**: TypeScript's structural subtyping tolerates extra fields, but Rust/Python/Go handlers have no such convention. Deserialization in strict-typed languages may reject unexpected fields or silently drop them.
3. **A bug vector**: a handler implementation reads `bar` dynamically, it works because `bar` happens to be in the pipeline data, nobody notices the schema doesn't declare `bar`, someone refactors the pipeline, `bar` disappears silently.

The fix: make TypedAction invariant in both In and Out. Require exact type matches at every step. When a handler needs a subset of the pipeline data, the caller inserts an explicit `pick` to narrow.

This is the TypeScript `Pick<T, K>` model: you explicitly select the fields you need before calling a function. No implicit tolerance of extra fields.

## Current state

TypedAction has three phantom positions for the input type:

```ts
// ast.ts
export type TypedAction<In, Out, Refs> = Action & {
  __phantom_in?: (input: In) => void;  // contravariant
  __phantom_out?: () => Out;            // covariant
  __in?: In;                            // covariant
  // ...
};
```

- `__phantom_in` (contravariant) ensures pipe chaining: output of step N is assignable to input of step N+1.
- `__in` (covariant) exists for two reasons: (a) entry point check — `config()` rejects workflows that expect input, and (b) inference help — gives TypeScript a covariant site to resolve intermediate types.
- Together, `__phantom_in` + `__in` create **invariance on In**.

`ChainableAction` omits `__in`, giving only contravariant input checking:

```ts
// ast.ts
export type ChainableAction<In, Out, Refs> = Action & {
  __phantom_in?: (input: In) => void;  // contravariant only
  __phantom_out?: () => Out;
  // no __in — deliberately omitted
};
```

`ChainableAction` exists solely to prevent invariance from propagating through `.then()`. The `.then()` method takes `ChainableAction<Out, TNext>` instead of `TypedAction<Out, TNext>`. If it took TypedAction, Out would appear in both covariant (`__in` of next) and contravariant (`__phantom_in` of next) positions, making Out invariant on the outer TypedAction. This was considered undesirable because it prevents handlers that produce extra fields from being piped to handlers that expect fewer fields.

With the decision to require exact matches: **Out being invariant is exactly what we want.** `ChainableAction` becomes unnecessary.

### Where ChainableAction is used

| Location | Usage |
|---|---|
| `ast.ts:130` | `.then()` parameter type |
| `builtins.ts:194` | `withResource` `create` parameter |
| `builtins.ts:196` | `withResource` `action` parameter |
| `builtins.ts:198` | `withResource` `dispose` parameter |

### Where `__in?: any` bypass is used

| Location | Usage |
|---|---|
| `builtins.ts:250` | `withResource` return type cast |

### Where `any` is used in TypedAction positions to dodge variance

| Location | Usage |
|---|---|
| `builtins.ts:196` | `action: ChainableAction<any, TOut>` in `withResource` |
| `builtins.ts:198` | `dispose: ChainableAction<NoInfer<TResource>, any>` |
| `builtins.ts:150` | `dropResult(action: TypedAction<TInput, any>)` |
| `builtins.ts:277,315` | `augment` and `tap` action params |
| `ast.ts:441` | `branch` cases constraint |
| `ast.ts:476` | `stepRef` return type |

## Proposed changes

### 1. Delete `ChainableAction`

Remove the type entirely from `ast.ts`. It exists only to work around variance — with invariance desired, the workaround is the bug.

**Files**: `ast.ts:166-174` (type definition), `builtins.ts:1` (import)

### 2. Change `.then()` to take `TypedAction`

```ts
// Before
then<TNext, TRefs2 extends string = never>(
  next: ChainableAction<Out, TNext, TRefs2>,
): TypedAction<In, TNext, Refs | TRefs2>;

// After
then<TNext, TRefs2 extends string = never>(
  next: TypedAction<Out, TNext, TRefs2>,
): TypedAction<In, TNext, Refs | TRefs2>;
```

This naturally makes Out invariant on TypedAction. Combined with In already being invariant, both type positions are now invariant.

**Consequence**: `pipe(a, b)` requires `a`'s Out to exactly equal `b`'s In. `pipe(setup, build)` works because `setup` outputs `{ initialized: boolean, project: string }` and `build` expects `{ initialized: boolean, project: string }`. But `pipe(producesBigObject, expectsSubset)` is rejected — the caller must insert `pick`.

**File**: `ast.ts:129-131`

### 3. Add `pick` builtin

A new builtin that extracts named fields from an object, producing an object with only those fields.

#### TypeScript side

```ts
// builtins.ts

export function pick<
  TObj extends Record<string, unknown>,
  TKeys extends (keyof TObj & string)[],
>(...keys: TKeys): TypedAction<TObj, Pick<TObj, TKeys[number]>> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Pick", value: keys } },
  });
}
```

Type inference: in `pipe(a, pick("foo", "bar"))`, TypeScript infers `TObj` from `a`'s output type via pipe's overload connecting the types. `TKeys` is inferred from the string literal arguments.

#### Postfix form

```ts
// On TypedAction:
pick<TKeys extends (keyof Out & string)[]>(
  ...keys: TKeys
): TypedAction<In, Pick<Out, TKeys[number]>, Refs>;
```

#### AST side

```ts
// ast.ts BuiltinKind union
| { kind: "Pick"; value: string[] }
```

#### Rust side

```rust
// barnum_ast BuiltinKind enum
Pick {
    /// The field names to keep (must be an array of JSON strings).
    value: Value,
},
```

Runtime: given input object and field names, produce a new object with only those fields. Error if input is not an object or a field doesn't exist.

### 4. Update `withResource`

With invariance, `withResource` can declare its action parameter's exact input type:

```ts
export function withResource<
  TIn extends Record<string, unknown>,
  TResource extends Record<string, unknown>,
  TOut,
>({
  create,
  action,
  dispose,
}: {
  create: TypedAction<TIn, TResource>;
  action: TypedAction<TResource & TIn, TOut>;
  dispose: TypedAction<TResource, unknown>;
}): TypedAction<TIn, TOut> {
  // ... (internal merge unchanged — still parallel + merge internally)
}
```

Changes from current:
- `create`: `ChainableAction<TIn, TResource>` → `TypedAction<TIn, TResource>`. No more NoInfer — the user must ensure TIn matches the pipeline exactly (use `pick` if needed).
- `action`: `ChainableAction<any, TOut>` → `TypedAction<TResource & TIn, TOut>`. The merged type is explicit. If the action handler only needs some fields, the caller wraps it: `pipe(pick("worktreePath", "description"), implement)`.
- `dispose`: `ChainableAction<NoInfer<TResource>, any>` → `TypedAction<TResource, unknown>`. No more NoInfer, no more `any` output. Dispose receives the resource; its output is discarded (but typed as `unknown`, not `any`).
- Return type: `TypedAction<TIn, TOut>` — no more `& { __in?: any }` hack. With invariance desired, the return type must match the pipeline exactly. If TIn is narrower than the pipeline data, the caller wraps `withResource` in `pipe(pick(...), withResource(...))`.

**File**: `builtins.ts:185-251`

### 5. Update `tap`

Currently `tap` accepts `TypedAction<any, any, any>` for the side-effectful action. With invariance:

```ts
export function tap<TInput extends Record<string, unknown>>(
  action: TypedAction<TInput, unknown>,
): TypedAction<TInput, TInput> {
  // ...
}
```

The action must accept exactly `TInput`. If a handler needs a subset, the caller narrows:

```ts
// Before (tolerant)
tap(implement)

// After (invariant)
tap(pipe(pick("worktreePath", "description"), implement))
```

**File**: `builtins.ts:312-323`

### 6. Update `augment`

Currently `augment` accepts `TypedAction<any, TOutput>`. With invariance:

```ts
export function augment<
  TInput extends Record<string, unknown>,
  TOutput extends Record<string, unknown>,
  TRefs extends string = never,
>(
  action: TypedAction<TInput, TOutput, TRefs>,
): TypedAction<TInput, TInput & TOutput, TRefs> {
  // ...
}
```

The action must accept exactly `TInput`. Usage:

```ts
// Before (tolerant)
augment(pipe(extractField("file"), migrate))

// After (invariant) — action must accept the full input type
augment(pipe(pick("file"), migrate))
// ... hmm, this doesn't work because pick("file") outputs { file: string }
// and migrate expects { file: string }, which is fine. But augment needs
// the action's input to be TInput (the full pipeline type), not a subset.
```

Actually, `augment` has a tension: the action receives the full pipeline input but might only need a subset. With invariance, the action's declared input must match TInput exactly. A handler like `migrate` (which expects `{ file: string }`) can't be used directly because TInput might be `{ file: string, outputPath: string }`.

The solution: `augment` should be parameterized differently. The user explicitly narrows inside the augment:

```ts
augment(pipe(pick("file"), migrate))
```

But then the pipe inside augment goes `TInput → { file: string } → { file: string, migrated: boolean }`. Augment needs the action to start from TInput and produce TOutput. With invariance, `pick("file")` takes TInput and outputs `{ file: string }`, then `migrate` takes `{ file: string }` — types match at every step. Augment's internal `parallel(action, identity())` produces `[TOutput, TInput]`, then merge produces `TOutput & TInput`. This works — augment's action param can be `TypedAction<TInput, TOutput>` and the pipe inside handles the narrowing.

No change needed to augment's signature — the caller just inserts `pick` inside the pipe.

### 7. Update `dropResult`

```ts
// Before
export function dropResult<TInput>(
  action: TypedAction<TInput, any>,
): TypedAction<TInput, never>

// After
export function dropResult<TInput, TOutput>(
  action: TypedAction<TInput, TOutput>,
): TypedAction<TInput, never>
```

No more `any` in the output position. TOutput is inferred but unused.

**File**: `builtins.ts:149-153`

### 8. Update tests

Several type tests use explicit type parameters on builtins like `extractField<..., "errors">("errors")`. With invariance, pipe will enforce exact matches, so some tests may need `pick` inserted.

The `@ts-expect-error` tests for mismatched types should continue to work (they already test that mismatches are rejected — invariance makes this stricter).

**Files**: `tests/types.test.ts`, `tests/patterns.test.ts`

### 9. Update demos

Both demos need `pick` inserted where handlers receive a subset of the pipeline:

**`demos/identify-and-address-refactors/run.ts`**:
- `tap(implement)` → `tap(pipe(pick("worktreePath", "description"), implement))`
- `tap(commit)` → `tap(pipe(pick("worktreePath"), commit))`
- Other handler calls that expect subsets

**`demos/convert-folder-to-ts/run.ts`**:
- Audit for handler calls that expect subsets

### 10. Remove dead code

After all changes:
- Delete `ChainableAction` type definition (`ast.ts:157-174`)
- Delete all comments explaining `ChainableAction`'s purpose (`ast.ts:117-128`, etc.)
- Remove `ChainableAction` from imports
- Remove `__in?: any` cast in `withResource`
- Remove `NoInfer` usage (no longer needed — exact matches mean no narrowing issues)

## Interaction with `let` bindings

With invariance + `let` bindings (LET_BINDINGS.md), the `pick` pattern inside `tap` and `augment` may become less necessary. Variable references provide named access to specific values without threading the full pipeline object through every step:

```ts
let_({
  resource: pipe(deriveBranch, createWorktree),
}, ({ resource }) =>
  pipe(
    // resource is a precise reference — no extra fields to worry about
    resource.then(implement),
    resource.then(commit),
  ),
)
```

Variables don't carry extra fields because they're individually bound. This may reduce the verbosity cost of invariance.

## Open questions

1. **Should `branch` case inputs be invariant?** Currently each branch case receives `{ kind: K }` (just the tag). If the original tagged union was `{ kind: "HasErrors", errors: string[] } | { kind: "Clean" }`, the HasErrors case receives `{ kind: "HasErrors", errors: string[] }` — including the value field. With invariance, the case handler must accept exactly this type. This seems fine.

2. **`stepRef` return type**: Currently `TypedAction<any, any, N>`. Since step references are for mutual recursion (types unknown at definition time), they may need to remain `any`. This is a deliberate escape hatch, not a variance workaround.

3. **Inference regression**: The `__in` covariant field helps TypeScript infer intermediate types in pipe chains (e.g., `extractField("errors")` without explicit type params). With Out also invariant, inference may actually improve in some cases (more constraints = more inference sites) or regress in others. Needs testing.

4. **Engine-level Pick**: Deferred to DEFERRED_FEATURES.md. The engine could enforce schema-based input filtering at handler boundaries as defense-in-depth, but the type system should be the primary enforcement mechanism.
