import {
  type Action,
  type MergeTuple,
  type Option as OptionT,
  type Pipeable,
  type Result as ResultT,
  type TaggedUnion,
  type TypedAction,
  typedAction,
  branch,
} from "./ast.js";
import { chain } from "./chain.js";
import { all } from "./all.js";
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

export const drop: TypedAction<any, never> = typedAction({
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
// Tap — run an action for side effects, preserve original input
// ---------------------------------------------------------------------------

/**
 * Run `action` on the input for its side effects, then discard the action's
 * output and return the original input unchanged. The action must accept
 * exactly `TInput`. Use `pick` inside the action's pipe if the inner
 * handler needs a subset.
 *
 * Constraint: input must be an object (uses all + merge internally).
 *
 * Example:
 *   pipe(tap(pipe(pick("worktreePath", "description"), implement)), createPR)
 */
export function tap<TInput extends Record<string, unknown>>(
  action: Pipeable<TInput, any>,
): TypedAction<TInput, TInput> {
  // all(chain(action, constant({})), identity) → merge
  return chain(
    all(chain(action, constant({})), identity()) as any,
    merge(),
  ) as TypedAction<TInput, TInput>;
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
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "SplitFirst" } },
  });
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
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "SplitLast" } },
  });
}

// ---------------------------------------------------------------------------
// First — extract the first element of an array as Option<TElement>
// ---------------------------------------------------------------------------

/**
 * Extract the first element of an array.
 * `readonly TElement[] → Option<TElement>`
 *
 * Composes `splitFirst` (which returns `Option<[TElement, TElement[]]>`)
 * with `Option.map(getIndex(0))` to extract just the element.
 */
export function first<TElement>(): TypedAction<
  readonly TElement[],
  OptionT<TElement>
> {
  return chain(
    splitFirst() as any,
    Option.map(getIndex(0) as any),
  ) as TypedAction<readonly TElement[], OptionT<TElement>>;
}

// ---------------------------------------------------------------------------
// Last — extract the last element of an array as Option<TElement>
// ---------------------------------------------------------------------------

/**
 * Extract the last element of an array.
 * `readonly TElement[] → Option<TElement>`
 *
 * Composes `splitLast` (which returns `Option<[TElement[], TElement]>`)
 * with `Option.map(getIndex(1))` to extract just the element.
 */
export function last<TElement>(): TypedAction<
  readonly TElement[],
  OptionT<TElement>
> {
  return chain(
    splitLast() as any,
    Option.map(getIndex(1) as any),
  ) as TypedAction<readonly TElement[], OptionT<TElement>>;
}

// ---------------------------------------------------------------------------
// Option namespace — combinators for Option<T> tagged unions
// ---------------------------------------------------------------------------

/**
 * Option namespace. All combinators produce TypedAction AST nodes that
 * desugar to branch + existing builtins, except collect which uses the
 * CollectSome builtin.
 */
export const Option = {
  /** Wrap a value as Some. `T → Option<T>` */
  some<T>(): TypedAction<T, OptionT<T>> {
    return tag("Some") as TypedAction<T, OptionT<T>>;
  },

  /** Produce a None. `never → Option<T>` */
  none<T>(): TypedAction<never, OptionT<T>> {
    return tag("None") as TypedAction<never, OptionT<T>>;
  },

  /** Transform the Some value. `Option<T> → Option<U>` */
  map<T, U>(action: Pipeable<T, U>): TypedAction<OptionT<T>, OptionT<U>> {
    return branch({
      Some: chain(action as any, tag("Some")),
      None: tag("None"),
    }) as TypedAction<OptionT<T>, OptionT<U>>;
  },

  /**
   * Monadic bind (flatMap). If Some, pass the value to action which
   * returns Option<U>. If None, stay None. `Option<T> → Option<U>`
   */
  andThen<T, U>(
    action: Pipeable<T, OptionT<U>>,
  ): TypedAction<OptionT<T>, OptionT<U>> {
    return branch({
      Some: action,
      None: tag("None"),
    }) as TypedAction<OptionT<T>, OptionT<U>>;
  },

  /**
   * Extract the Some value or produce a default from an action.
   * `Option<T> → T`
   *
   * The None branch drops its void payload before calling defaultAction,
   * matching Rust's `unwrap_or_else(|| default)`.
   */
  unwrapOr<T>(defaultAction: Pipeable<never, T>): TypedAction<OptionT<T>, T> {
    return branch({
      Some: identity(),
      None: chain(drop, defaultAction),
    }) as TypedAction<OptionT<T>, T>;
  },

  /** Unwrap a nested Option. `Option<Option<T>> → Option<T>` */
  flatten<T>(): TypedAction<OptionT<OptionT<T>>, OptionT<T>> {
    return branch({
      Some: identity(),
      None: tag("None"),
    }) as TypedAction<OptionT<OptionT<T>>, OptionT<T>>;
  },

  /**
   * Conditional keep. If Some, pass value to predicate which returns
   * Option<T>. If None, stay None. `Option<T> → Option<T>`
   */
  filter<T>(
    predicate: Pipeable<T, OptionT<T>>,
  ): TypedAction<OptionT<T>, OptionT<T>> {
    return branch({
      Some: predicate,
      None: tag("None"),
    }) as TypedAction<OptionT<T>, OptionT<T>>;
  },

  /**
   * Collect Some values from an array, discarding Nones.
   * `Option<T>[] → T[]`
   */
  collect<T = any>(): TypedAction<OptionT<T>[], T[]> {
    return typedAction({
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "CollectSome" } },
    });
  },

  /** Test if the value is Some. `Option<T> → boolean` */
  isSome<T>(): TypedAction<OptionT<T>, boolean> {
    return branch({
      Some: chain(drop, constant(true)),
      None: chain(drop, constant(false)),
    }) as TypedAction<OptionT<T>, boolean>;
  },

  /** Test if the value is None. `Option<T> → boolean` */
  isNone<T>(): TypedAction<OptionT<T>, boolean> {
    return branch({
      Some: chain(drop, constant(false)),
      None: chain(drop, constant(true)),
    }) as TypedAction<OptionT<T>, boolean>;
  },

  /**
   * Build a Zod schema for `Option<T>`.
   *
   * ```ts
   * const schema = Option.schema(z.string());
   * // validates: { kind: "Some", value: "hello" } or { kind: "None", value: null }
   * ```
   */
  schema<TValue>(valueSchema: z.ZodType<TValue>): z.ZodType<OptionT<TValue>> {
    return z.discriminatedUnion("kind", [
      z.object({ kind: z.literal("Some"), value: valueSchema }),
      z.object({ kind: z.literal("None"), value: z.null() }),
    ]) as z.ZodType<OptionT<TValue>>;
  },
} as const;

// ---------------------------------------------------------------------------
// Result namespace — combinators for Result<TValue, TError> tagged unions
// ---------------------------------------------------------------------------

export const Result = {
  /** Wrap a value as Ok. `TValue → Result<TValue, TError>` */
  ok<TValue, TError>(): TypedAction<TValue, ResultT<TValue, TError>> {
    return tag("Ok") as TypedAction<TValue, ResultT<TValue, TError>>;
  },

  /** Wrap a value as Err. `TError → Result<TValue, TError>` */
  err<TValue, TError>(): TypedAction<TError, ResultT<TValue, TError>> {
    return tag("Err") as TypedAction<TError, ResultT<TValue, TError>>;
  },

  /** Transform the Ok value. `Result<TValue, TError> → Result<TOut, TError>` */
  map<TValue, TOut, TError>(
    action: Pipeable<TValue, TOut>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return branch({
      Ok: chain(action as any, tag("Ok")),
      Err: tag("Err"),
    }) as TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>>;
  },

  /** Transform the Err value. `Result<TValue, TError> → Result<TValue, TErrorOut>` */
  mapErr<TValue, TError, TErrorOut>(
    action: Pipeable<TError, TErrorOut>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>> {
    return branch({
      Ok: tag("Ok"),
      Err: chain(action as any, tag("Err")),
    }) as TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>>;
  },

  /**
   * Monadic bind (flatMap) for Ok. If Ok, pass value to action which
   * returns Result<TOut, TError>. If Err, propagate.
   */
  andThen<TValue, TOut, TError>(
    action: Pipeable<TValue, ResultT<TOut, TError>>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return branch({
      Ok: action,
      Err: tag("Err"),
    }) as TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>>;
  },

  /** Fallback on Err. If Ok, keep it. If Err, pass error to fallback. */
  or<TValue, TError, TErrorOut>(
    fallback: Pipeable<TError, ResultT<TValue, TErrorOut>>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>> {
    return branch({
      Ok: tag("Ok"),
      Err: fallback,
    }) as TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>>;
  },

  /** Replace Ok value with another Result. If Ok, discard value and return other. */
  and<TValue, TOut, TError>(
    other: Pipeable<never, ResultT<TOut, TError>>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return branch({
      Ok: chain(drop, other),
      Err: tag("Err"),
    }) as TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>>;
  },

  /**
   * Extract Ok or compute default from Err. `Result<TValue, TError> → TValue`
   *
   * Uses covariant output checking so throw tokens (Out=never) are assignable.
   * For inference-free usage with throw tokens, prefer the postfix method:
   * `handler.unwrapOr(throwError)`.
   */
  unwrapOr<TValue, TError>(
    defaultAction: Action & {
      __in?: (input: TError) => void;
      __out?: () => TValue;
    },
  ): TypedAction<ResultT<TValue, TError>, TValue> {
    return branch({
      Ok: identity(),
      Err: defaultAction,
    }) as TypedAction<ResultT<TValue, TError>, TValue>;
  },

  /** Unwrap nested Result. `Result<Result<TValue, TError>, TError> → Result<TValue, TError>` */
  flatten<TValue, TError>(): TypedAction<
    ResultT<ResultT<TValue, TError>, TError>,
    ResultT<TValue, TError>
  > {
    return branch({
      Ok: identity(),
      Err: tag("Err"),
    }) as TypedAction<
      ResultT<ResultT<TValue, TError>, TError>,
      ResultT<TValue, TError>
    >;
  },

  /** Convert Ok to Some, Err to None. `Result<TValue, TError> → Option<TValue>` */
  toOption<TValue, TError>(): TypedAction<
    ResultT<TValue, TError>,
    OptionT<TValue>
  > {
    return branch({
      Ok: tag("Some"),
      Err: drop.tag("None"),
    }) as TypedAction<ResultT<TValue, TError>, OptionT<TValue>>;
  },

  /** Convert Err to Some, Ok to None. `Result<TValue, TError> → Option<TError>` */
  toOptionErr<TValue, TError>(): TypedAction<
    ResultT<TValue, TError>,
    OptionT<TError>
  > {
    return branch({
      Ok: drop.tag("None"),
      Err: tag("Some"),
    }) as TypedAction<ResultT<TValue, TError>, OptionT<TError>>;
  },

  /**
   * Swap Result/Option nesting.
   * `Result<Option<TValue>, TError> → Option<Result<TValue, TError>>`
   */
  transpose<TValue, TError>(): TypedAction<
    ResultT<OptionT<TValue>, TError>,
    OptionT<ResultT<TValue, TError>>
  > {
    return branch({
      Ok: branch({
        Some: chain(tag("Ok") as any, tag("Some")),
        None: drop.tag("None"),
      }),
      Err: chain(tag("Err") as any, tag("Some")),
    }) as TypedAction<
      ResultT<OptionT<TValue>, TError>,
      OptionT<ResultT<TValue, TError>>
    >;
  },

  /** Test if the value is Ok. `Result<TValue, TError> → boolean` */
  isOk<TValue, TError>(): TypedAction<ResultT<TValue, TError>, boolean> {
    return branch({
      Ok: chain(drop, constant(true)),
      Err: chain(drop, constant(false)),
    }) as TypedAction<ResultT<TValue, TError>, boolean>;
  },

  /** Test if the value is Err. `Result<TValue, TError> → boolean` */
  isErr<TValue, TError>(): TypedAction<ResultT<TValue, TError>, boolean> {
    return branch({
      Ok: chain(drop, constant(false)),
      Err: chain(drop, constant(true)),
    }) as TypedAction<ResultT<TValue, TError>, boolean>;
  },

  /**
   * Build a Zod schema for `Result<TValue, TError>`.
   *
   * ```ts
   * const schema = Result.schema(z.string(), z.number());
   * // validates: { kind: "Ok", value: "hello" } or { kind: "Err", value: 42 }
   * ```
   */
  schema<TValue, TError>(
    okSchema: z.ZodType<TValue>,
    errSchema: z.ZodType<TError>,
  ): z.ZodType<ResultT<TValue, TError>> {
    return z.discriminatedUnion("kind", [
      z.object({ kind: z.literal("Ok"), value: okSchema }),
      z.object({ kind: z.literal("Err"), value: errSchema }),
    ]) as z.ZodType<ResultT<TValue, TError>>;
  },
} as const;
