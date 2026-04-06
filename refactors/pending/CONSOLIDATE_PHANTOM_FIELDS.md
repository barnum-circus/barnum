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

## Design

A function type `(x: T) => T` is **invariant** in `T`: `T` appears in both parameter (contravariant) and return (covariant) position. A function `(x: A) => B` is contravariant in `A` and covariant in `B`.

### TypedAction and Pipeable: invariant In, invariant Out

Replace four fields with one:

```ts
__phantom?: (io: [In, Out]) => [In, Out];
```

`[In, Out]` appears in both parameter and return position, making the tuple (and therefore both `In` and `Out`) invariant.

### CaseHandler: contravariant In, covariant Out

Replace two fields with one:

```ts
__phantom?: (input: In) => Out;
```

`In` in parameter position (contravariant), `Out` in return position (covariant).

### ExtractInput / ExtractOutput

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

Both extract from the same `__phantom` field. `any` in the non-extracted positions avoids interference.

Note: CaseHandler's `(input: In) => Out` signature is a *subtype* of `(io: [In, Out]) => [In, Out]`? No — these are different shapes. But both TypedAction and CaseHandler use `__phantom` as the field name, and TypeScript's structural matching in `extends` clauses will match whichever shape fits the `infer` pattern.

Actually, this is a problem. `ExtractInput` matches `__phantom?: (io: [infer In, any]) => any`. CaseHandler's `__phantom?: (input: In) => Out` doesn't match that pattern because its parameter isn't a tuple. So extraction from CaseHandler would return `never`.

But we never call `ExtractInput` or `ExtractOutput` on a CaseHandler directly — they're only used on TypedAction/Pipeable. CaseHandler is an internal constraint type. Let me verify.

## Extraction compatibility

`ExtractInput<T>` and `ExtractOutput<T>` are used in:
- Type test assertions (`assertExact<IsExact<ExtractInput<typeof action>, ...>>()`)
- Branch output extraction (`ExtractOutput<TCases[keyof TCases & string]>`)
- The `BranchPayload` type utility

The branch output extraction uses `ExtractOutput` on the *values* of the cases object, which are `CaseHandler` types at the constraint level but `TypedAction`/`Pipeable` at the value level. Since concrete handlers are TypedAction instances, `ExtractOutput` will see the tuple-based `__phantom` and work correctly.

So the approach is safe: CaseHandler uses `(input: In) => Out` for variance, TypedAction/Pipeable use `(io: [In, Out]) => [In, Out]` for variance and extraction. The field name `__phantom` is shared but the shapes differ. Extraction only needs to work on TypedAction/Pipeable, which it does.

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

No runtime changes needed — it's just a cast.

## Changes

### 1. Replace phantom fields on TypedAction

**Before:**
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

**After:**
```ts
export type TypedAction<
  In = unknown,
  Out = unknown,
  Refs extends string = never,
> = Action & {
  __phantom?: (io: [In, Out]) => [In, Out];
  __refs?: { _brand: Refs };
  // ...methods...
};
```

### 2. Replace phantom fields on Pipeable

Same change — four fields become one `__phantom`.

### 3. Replace phantom fields on CaseHandler

**Before:**
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

**After:**
```ts
type CaseHandler<
  TIn = unknown,
  TOut = unknown,
  TRefs extends string = never,
> = Action & {
  __phantom?: (input: TIn) => TOut;
  __refs?: { _brand: TRefs };
};
```

### 4. Update ExtractInput / ExtractOutput

**Before:**
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

### 5. Update JSDoc comments

Replace the variance explanation in the TypedAction, Pipeable, and CaseHandler JSDoc blocks. The new explanation:

For TypedAction/Pipeable: "A single phantom field `__phantom?: (io: [In, Out]) => [In, Out]` encodes invariance — the tuple appears in both parameter (contravariant) and return (covariant) position."

For CaseHandler: "A single phantom field `__phantom?: (input: TIn) => TOut` encodes contravariant input and covariant output — `TIn` in parameter position, `TOut` in return position."

### 6. Update docs

Update `docs-website/docs/architecture/typescript-ast.md` if it references the phantom field names.

## Verification

All existing type tests in `types.test.ts` exercise the variance constraints:
- Pipe rejects mismatched types (invariant)
- Branch accepts `drop` as a case handler (contravariant input on CaseHandler)
- Branch rejects wrong handler type (contravariant input on CaseHandler)
- `ExtractInput` / `ExtractOutput` assertions throughout

If the consolidated fields pass the existing type tests, the variance behavior is preserved.

## Execution order

1. Replace phantom fields on TypedAction, Pipeable, CaseHandler
2. Update ExtractInput / ExtractOutput
3. Update JSDoc comments
4. Typecheck + run tests
5. Update docs
