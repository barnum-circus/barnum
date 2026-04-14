import {
  type Action,
  type MergeTuple,
  type Option as OptionT,
  type Pipeable,
  type TaggedUnion,
  type TypedAction,
  typedAction,
  withUnion,
} from "./ast.js";
import { chain } from "./chain.js";
import { all } from "./all.js";
// Lazy: optionMethods is only accessed inside function bodies, not at module init.
import { optionMethods } from "./option.js";
import { z } from "zod";

/**
 * Typed combinators for structural data transformations.
 *
 * All builtins emit `{ kind: "Builtin", builtin: { kind: ... } }` handler
 * kinds. The Rust scheduler executes them inline (no subprocess).
 */

// ---------------------------------------------------------------------------
// TaggedUnion Zod schema constructor
// ---------------------------------------------------------------------------

/**
 * Reverse of VoidToNull: maps `null` back to `void` in the def so that
 * `taggedUnionSchema({ Clean: z.null() })` produces the same phantom __def
 * as `TaggedUnion<{ Clean: void }>`.
 */
type NullToVoid<TDef> = {
  [K in keyof TDef]: TDef[K] extends null ? void : TDef[K];
};

/**
 * Build a Zod schema for a `TaggedUnion<TDef>` — a discriminated union of
 * `{ kind: K; value: V }` objects.
 *
 * Each key in `cases` becomes a variant. The value Zod schema validates the
 * `value` field. Use `z.null()` for void variants.
 *
 * ```ts
 * const schema = taggedUnionSchema({
 *   HasErrors: z.array(TypeErrorValidator),
 *   Clean: z.null(),
 * });
 * ```
 */
export function taggedUnionSchema<TDef extends Record<string, z.ZodTypeAny>>(
  cases: TDef,
): z.ZodType<
  TaggedUnion<NullToVoid<{ [K in keyof TDef & string]: z.infer<TDef[K]> }>>
> {
  type Out = TaggedUnion<
    NullToVoid<{ [K in keyof TDef & string]: z.infer<TDef[K]> }>
  >;
  const variants = Object.entries(cases).map(([kind, valueSchema]) =>
    z.object({ kind: z.literal(kind), value: valueSchema }),
  );
  if (variants.length === 0) {
    return z.never() as z.ZodType<Out>;
  }
  if (variants.length === 1) {
    return variants[0] as z.ZodType<Out>;
  }
  return z.discriminatedUnion("kind", [
    variants[0],
    variants[1],
    ...variants.slice(2),
  ]) as z.ZodType<Out>;
}

// ---------------------------------------------------------------------------
// Constant — produce a fixed value (takes no pipeline input)
// ---------------------------------------------------------------------------

export function constant<TValue>(value: TValue): TypedAction<any, TValue> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Constant", value } },
  });
}

// ---------------------------------------------------------------------------
// Identity — pass input through unchanged
// ---------------------------------------------------------------------------

export function identity<TValue = any>(): TypedAction<TValue, TValue> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Identity" } },
  });
}

// ---------------------------------------------------------------------------
// Drop — discard pipeline value
// ---------------------------------------------------------------------------

export const drop: TypedAction<any, void> = typedAction({
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Drop" } },
});

// ---------------------------------------------------------------------------
// Tag — wrap input as a tagged union variant
// ---------------------------------------------------------------------------

/**
 * Wrap input as a tagged union member. Requires the full variant map TDef
 * so the output type carries __def for branch decomposition.
 *
 * Usage: tag<{ Ok: string; Err: number }, "Ok">("Ok")
 *        input: string → output: TaggedUnion<{ Ok: string; Err: number }>
 */
export function tag<
  TDef extends Record<string, unknown>,
  TKind extends keyof TDef & string,
>(kind: TKind): TypedAction<TDef[TKind], TaggedUnion<TDef>> {
  return chain(
    all(
      chain(constant(kind) as any, wrapInField("kind")),
      wrapInField("value"),
    ) as any,
    merge(),
  ) as TypedAction<TDef[TKind], TaggedUnion<TDef>>;
}

// ---------------------------------------------------------------------------
// Merge — merge a tuple of objects into a single object
// ---------------------------------------------------------------------------

export function merge<TTuple extends Record<string, unknown>[]>(): TypedAction<
  TTuple,
  MergeTuple<TTuple>
> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Merge" } },
  });
}

// ---------------------------------------------------------------------------
// Flatten — flatten a nested array one level
// ---------------------------------------------------------------------------

export function flatten<TElement>(): TypedAction<TElement[][], TElement[]> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Flatten" } },
  });
}

// ---------------------------------------------------------------------------
// GetField — extract a single field from an object
// ---------------------------------------------------------------------------

export function getField<
  TObj extends Record<string, unknown>,
  TField extends keyof TObj & string,
>(field: TField): TypedAction<TObj, TObj[TField]> {
  return typedAction({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: { kind: "GetField", field },
    },
  });
}

// ---------------------------------------------------------------------------
// GetIndex — extract a single element from an array by index
// ---------------------------------------------------------------------------

export function getIndex<TTuple extends unknown[], TIndex extends number>(
  index: TIndex,
): TypedAction<TTuple, TTuple[TIndex]> {
  return typedAction({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: { kind: "GetIndex", index },
    },
  });
}

// ---------------------------------------------------------------------------
// Pick — select named fields from an object
// ---------------------------------------------------------------------------

export function pick<
  TObj extends Record<string, unknown>,
  TKeys extends (keyof TObj & string)[],
>(...keys: TKeys): TypedAction<TObj, Pick<TObj, TKeys[number]>> {
  const actions = keys.map(
    (key) => chain(getField(key) as any, wrapInField(key)) as Action,
  );
  const allAction: Action = { kind: "All", actions };
  return chain(allAction as any, merge() as any) as TypedAction<
    TObj,
    Pick<TObj, TKeys[number]>
  >;
}

// ---------------------------------------------------------------------------
// WithResource — RAII-style create/action/dispose
// ---------------------------------------------------------------------------

/**
 * RAII-style resource management combinator.
 *
 * Runs `create` to acquire a resource, then merges the resource with the
 * original input into a flat object (`TResource & TIn`) for the action.
 * After the action completes, `dispose` receives the resource for cleanup.
 * The overall combinator returns the action's output.
 *
 * ```
 * TIn → create → TResource
 *     → merge(TResource, TIn) → TResource & TIn
 *     → action(TResource & TIn) → TOut
 *     → dispose(TResource) → (discarded)
 *     → TOut
 * ```
 */
export function withResource<
  TIn extends Record<string, unknown>,
  TResource extends Record<string, unknown>,
  TOut,
  TDisposeOut = unknown,
>({
  create,
  action,
  dispose,
}: {
  create: Pipeable<TIn, TResource>;
  action: Pipeable<TResource & TIn, TOut>;
  dispose: Pipeable<TResource, TDisposeOut>;
}): TypedAction<TIn, TOut> {
  // Step 1: all(create, identity) → [TResource, TIn] → merge → TResource & TIn
  const acquireAndMerge = chain(all(create, identity()) as any, merge());

  // Step 2: all(action, identity) → [TOut, TResource & TIn]
  const actionAndKeepMerged = all(action as any, identity());

  // Step 3: all(getIndex(0), chain(getIndex(1), dispose)) → [TOut, unknown]
  const disposeAndKeepResult = all(
    getIndex(0) as any,
    chain(getIndex(1) as any, dispose),
  );

  // Step 4: getIndex(0) → TOut
  return chain(
    chain(
      chain(acquireAndMerge, actionAndKeepMerged) as any,
      disposeAndKeepResult,
    ),
    getIndex(0) as any,
  ) as TypedAction<TIn, TOut>;
}

// ---------------------------------------------------------------------------
// WrapInField — wrap input as { <field>: <input> }
// ---------------------------------------------------------------------------

export function wrapInField<TField extends string, TValue>(
  field: TField,
): TypedAction<TValue, Record<TField, TValue>> {
  return typedAction({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: { kind: "WrapInField", field },
    },
  });
}

// ---------------------------------------------------------------------------
// Range — produce an integer array [start, start+1, ..., end-1]
// ---------------------------------------------------------------------------

export function range(start: number, end: number): TypedAction<any, number[]> {
  const result: number[] = [];
  for (let i = start; i < end; i++) {
    result.push(i);
  }
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Constant", value: result } },
  });
}

// ---------------------------------------------------------------------------
// SplitFirst — head/tail decomposition of an array
// ---------------------------------------------------------------------------

/**
 * Deconstruct an array into its first element and the remaining elements.
 * `TElement[] → Option<[TElement, TElement[]]>`
 *
 * Returns `Some([first, rest])` for non-empty arrays, `None` for empty arrays.
 * This is the array equivalent of cons/uncons — enables recursive iteration
 * patterns via `loop` + `splitFirst` + `branch`.
 *
 * This is a builtin (SplitFirst) because it requires array-length branching
 * that can't be composed from existing AST nodes.
 */
export function splitFirst<TElement>(): TypedAction<
  TElement[],
  OptionT<[TElement, TElement[]]>
> {
  return withUnion(
    typedAction({
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "SplitFirst" } },
    }),
    optionMethods,
  );
}

// ---------------------------------------------------------------------------
// SplitLast — init/last decomposition of an array
// ---------------------------------------------------------------------------

/**
 * Deconstruct an array into the leading elements and the last element.
 * `TElement[] → Option<[TElement[], TElement]>`
 *
 * Returns `Some([init, last])` for non-empty arrays, `None` for empty arrays.
 * Mirror of `splitFirst` — enables processing from the tail end.
 *
 * This is a builtin (SplitLast) because it requires array-length branching
 * that can't be composed from existing AST nodes.
 */
export function splitLast<TElement>(): TypedAction<
  TElement[],
  OptionT<[TElement[], TElement]>
> {
  return withUnion(
    typedAction({
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "SplitLast" } },
    }),
    optionMethods,
  );
}

