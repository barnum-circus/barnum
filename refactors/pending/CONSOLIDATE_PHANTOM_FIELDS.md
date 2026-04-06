# Consolidate Phantom Fields

## Motivation

`TypedAction`, `Pipeable`, and `CaseHandler` each carry four phantom fields to encode input/output variance:

```ts
__phantom_in?: (input: In) => void;   // contravariant In
__phantom_out?: () => Out;             // covariant Out
__phantom_out_check?: (output: Out) => void;  // contravariant Out
__in?: In;                             // covariant In
```

The first pair (`__phantom_in` + `__in`) makes In invariant. The second pair (`__phantom_out` + `__phantom_out_check`) makes Out invariant. `CaseHandler` drops `__in` and `__phantom_out_check` to get contravariant In / covariant Out.

Four fields with inconsistent naming (`__in` vs `__phantom_in`), a stale comment about the deleted `WorkflowAction`, and a non-obvious variance encoding. This can be one field.

Additionally, all three types carry a `Refs extends string = never` type parameter and `__refs?: { _brand: Refs }` phantom field. This is threaded through every combinator signature but never extracted or used — no `ExtractRefs` type exists, no test exercises it, no consumer reads it. Dead code.

## Design

### Step 1: Remove Refs

Delete the `Refs` type parameter and `__refs` field from `TypedAction`, `Pipeable`, and `CaseHandler`. Remove the `TRefs` parameter from every combinator signature that threads it (`then`, `forEach`, `pipe`, `chain`, `loop`, `recur`, `earlyReturn`, `dropResult`, `augment`, `tap`, `typedAction`, etc.).

### Step 2: Consolidate phantom fields

A function type `(x: T) => T` is **invariant** in `T`: `T` appears in both parameter (contravariant) and return (covariant) position. A function `(x: A) => B` is contravariant in `A` and covariant in `B`.

**TypedAction and Pipeable** (invariant In, invariant Out): replace four fields with one:

```ts
__phantom?: (io: [In, Out]) => [In, Out];
```

`[In, Out]` appears in both parameter and return position, making both `In` and `Out` invariant.

**CaseHandler** (contravariant In, covariant Out): replace two fields with one:

```ts
__phantom?: (input: TIn) => TOut;
```

`TIn` in parameter position (contravariant), `TOut` in return position (covariant).

**ExtractInput / ExtractOutput**: extract from the tuple shape:

```ts
export type ExtractInput<T> = T extends {
  __phantom?: (io: [infer In, any]) => any;
}
  ? In
  : never;

export type ExtractOutput<T> = T extends {
  __phantom?: (io: any) => [any, infer Out];
}
  ? Out
  : never;
```

CaseHandler's `(input: TIn) => TOut` doesn't match the tuple extraction pattern, but `ExtractInput`/`ExtractOutput` are never called on CaseHandler — only on concrete TypedAction/Pipeable values. CaseHandler is a constraint type for branch case positions.

## Current state

### `TypedAction` (`libs/barnum/src/ast.ts:151-159`)

```ts
export type TypedAction<
  In = unknown,
  Out = unknown,
  Refs extends string = never,
> = Action & {
  __phantom_in?: (input: In) => void;
  __phantom_out?: () => Out;
  __phantom_out_check?: (output: Out) => void;
  __in?: In;
  __refs?: { _brand: Refs };
  // ...methods...
};
```

### `Pipeable` (`libs/barnum/src/ast.ts:276-286`)

```ts
export type Pipeable<
  In = unknown,
  Out = unknown,
  Refs extends string = never,
> = Action & {
  __phantom_in?: (input: In) => void;
  __phantom_out?: () => Out;
  __phantom_out_check?: (output: Out) => void;
  __in?: In;
  __refs?: { _brand: Refs };
};
```

### `CaseHandler` (`libs/barnum/src/ast.ts:308-316`)

```ts
type CaseHandler<
  TIn = unknown,
  TOut = unknown,
  TRefs extends string = never,
> = Action & {
  __phantom_in?: (input: TIn) => void;
  __phantom_out?: () => TOut;
  __refs?: { _brand: TRefs };
};
```

### `ExtractInput` / `ExtractOutput` (`libs/barnum/src/ast.ts:634-647`)

```ts
export type ExtractInput<T> = T extends {
  __phantom_in?: (input: infer In) => void;
}
  ? In
  : never;

export type ExtractOutput<T> = T extends { __phantom_out?: () => infer Out }
  ? Out
  : never;
```

### `typedAction` (`libs/barnum/src/ast.ts:614-621`)

```ts
export function typedAction<In, Out, Refs extends string = never>(
  action: Action,
): TypedAction<In, Out, Refs> {
  return action as TypedAction<In, Out, Refs>;
}
```

## Changes

### 1. Remove `Refs` type parameter and `__refs` field

Remove from:
- `TypedAction<In, Out, Refs>` → `TypedAction<In, Out>`
- `Pipeable<In, Out, Refs>` → `Pipeable<In, Out>`
- `CaseHandler<TIn, TOut, TRefs>` → `CaseHandler<TIn, TOut>`
- `typedAction<In, Out, Refs>` → `typedAction<In, Out>`

Remove `TRefs` parameters from every combinator that threads them: `then`, `forEach`, `chain`/pipe implementation, `loop`, `recur`, `earlyReturn`, `dropResult`, `augment`, `tap`, and all method signatures on TypedAction.

Remove `__refs` field from all three types.

### 2. Replace phantom fields on TypedAction

**After:**
```ts
export type TypedAction<In = unknown, Out = unknown> = Action & {
  __phantom?: (io: [In, Out]) => [In, Out];
  // ...methods (without Refs)...
};
```

### 3. Replace phantom fields on Pipeable

**After:**
```ts
export type Pipeable<In = unknown, Out = unknown> = Action & {
  __phantom?: (io: [In, Out]) => [In, Out];
};
```

### 4. Replace phantom fields on CaseHandler

**After:**
```ts
type CaseHandler<TIn = unknown, TOut = unknown> = Action & {
  __phantom?: (input: TIn) => TOut;
};
```

### 5. Update ExtractInput / ExtractOutput

**After:**
```ts
export type ExtractInput<T> = T extends {
  __phantom?: (io: [infer In, any]) => any;
}
  ? In
  : never;

export type ExtractOutput<T> = T extends {
  __phantom?: (io: any) => [any, infer Out];
}
  ? Out
  : never;
```

### 6. Update JSDoc comments

Replace the variance explanation blocks. New text:

For TypedAction/Pipeable: "A single phantom field `__phantom?: (io: [In, Out]) => [In, Out]` encodes invariance — the tuple appears in both parameter (contravariant) and return (covariant) position."

For CaseHandler: "A single phantom field `__phantom?: (input: TIn) => TOut` encodes contravariant input and covariant output — `TIn` in parameter position, `TOut` in return position."

### 7. Update docs

Update `docs-website/docs/architecture/typescript-ast.md` if it references the phantom field names.

## Verification

All existing type tests in `types.test.ts` exercise the variance constraints:
- Pipe rejects mismatched types (invariant)
- Branch accepts `drop` as a case handler (contravariant input on CaseHandler)
- Branch rejects wrong handler type (contravariant input on CaseHandler)
- `ExtractInput` / `ExtractOutput` assertions throughout

If the consolidated fields pass the existing type tests, the variance behavior is preserved.

## Execution order

1. Remove `Refs` from all types and combinators
2. Replace phantom fields on TypedAction, Pipeable, CaseHandler
3. Update ExtractInput / ExtractOutput
4. Update JSDoc comments
5. Typecheck + run tests
6. Update docs
