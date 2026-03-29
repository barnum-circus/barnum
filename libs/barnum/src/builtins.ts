import { type Action, type LoopResult, type Option as OptionT, type OptionDef, type Pipeable, type TaggedUnion, type TypedAction, typedAction } from "./ast.js";
import { chain } from "./chain.js";

/**
 * Typed combinators for structural data transformations.
 *
 * All builtins emit `{ kind: "Builtin", builtin: { kind: ... } }` handler
 * kinds. The Rust scheduler executes them inline (no subprocess).
 */

// ---------------------------------------------------------------------------
// Constant — produce a fixed value (takes no pipeline input)
// ---------------------------------------------------------------------------

export function constant<TValue>(value: TValue): TypedAction<never, TValue> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Constant", value } },
  });
}

// ---------------------------------------------------------------------------
// Identity — pass input through unchanged
// ---------------------------------------------------------------------------

export function identity<TValue>(): TypedAction<TValue, TValue> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Identity" } },
  });
}

// ---------------------------------------------------------------------------
// Drop — discard pipeline value
// ---------------------------------------------------------------------------

export function drop<TValue>(): TypedAction<TValue, never> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Drop" } },
  });
}

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
>(
  kind: TKind,
): TypedAction<TDef[TKind], TaggedUnion<TDef>> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Tag", value: kind } },
  });
}

// ---------------------------------------------------------------------------
// Loop signals
//
// Both recur and done produce the full LoopResult<TContinue, TBreak> output
// type so the __def phantom field carries the complete variant map. Both type
// parameters are required — this ensures the branch output is a proper
// TaggedUnion with consistent __def across all cases.
// ---------------------------------------------------------------------------

export function recur<TContinue, TBreak>(): TypedAction<
  TContinue,
  LoopResult<TContinue, TBreak>
> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Continue" } },
  });
}

export function done<TContinue, TBreak>(): TypedAction<
  TBreak,
  LoopResult<TContinue, TBreak>
> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Break" } },
  });
}

// ---------------------------------------------------------------------------
// Merge — merge a tuple of objects into a single object
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type UnionToIntersection<U> = (U extends any ? (x: U) => void : never) extends (
  x: infer I,
) => void
  ? I
  : never;

export function merge<
  TObjects extends Record<string, unknown>[],
>(): TypedAction<TObjects, UnionToIntersection<TObjects[number]>> {
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
// ExtractField — extract a single field from an object
// ---------------------------------------------------------------------------

export function extractField<
  TObj extends Record<string, unknown>,
  TField extends keyof TObj & string,
>(field: TField): TypedAction<TObj, TObj[TField]> {
  return typedAction({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: { kind: "ExtractField", value: field },
    },
  });
}

// ---------------------------------------------------------------------------
// ExtractIndex — extract a single element from an array by index
// ---------------------------------------------------------------------------

export function extractIndex<
  TTuple extends unknown[],
  TIndex extends number,
>(index: TIndex): TypedAction<TTuple, TTuple[TIndex]> {
  return typedAction({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: { kind: "ExtractIndex", value: index },
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
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Pick", value: keys } },
  });
}

// ---------------------------------------------------------------------------
// DropResult — run an action for side effects, discard its output
// ---------------------------------------------------------------------------

export function dropResult<TInput, TOutput, TRefs extends string = never>(
  action: Pipeable<TInput, TOutput, TRefs>,
): TypedAction<TInput, never, TRefs> {
  // Build AST directly — chain inference fails when drop()'s generic TValue
  // isn't constrained by context (resolves to unknown ≠ TOutput).
  return typedAction({
    kind: "Chain",
    first: action as Action,
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Drop" } } },
  });
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
  const mergeBuiltin: Action = {
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Merge" } },
  };

  // Step 1: parallel(create, identity) → [TResource, TIn] → merge → TResource & TIn
  const acquireAndMerge = chain(
    typedAction<TIn, [TResource, TIn]>({
      kind: "Parallel",
      actions: [create as Action, identity() as Action],
    }),
    typedAction<[TResource, TIn], TResource & TIn>(mergeBuiltin),
  );

  // Step 2: parallel(action, identity) → [TOut, TResource & TIn]
  // Keep merged object so dispose can access resource fields.
  const actionAndKeepMerged = typedAction<TResource & TIn, [TOut, TResource & TIn]>({
    kind: "Parallel",
    actions: [action as Action, identity() as Action],
  });

  // Step 3: parallel(extractIndex(0), chain(extractIndex(1), dispose)) → [TOut, unknown]
  const disposeAndKeepResult = typedAction<[TOut, TResource & TIn], [TOut, unknown]>({
    kind: "Parallel",
    actions: [
      extractIndex<[TOut, TResource & TIn], 0>(0) as Action,
      chain(
        extractIndex<[TOut, TResource & TIn], 1>(1),
        dispose as Pipeable<TResource & TIn, unknown>,
      ) as Action,
    ],
  });

  // Step 4: extractIndex(0) → TOut
  return chain(
    chain(chain(acquireAndMerge, actionAndKeepMerged), disposeAndKeepResult),
    extractIndex<[TOut, unknown], 0>(0),
  ) as TypedAction<TIn, TOut>;
}

// ---------------------------------------------------------------------------
// Augment — run a transform, merge its output back into the original input
// ---------------------------------------------------------------------------

/**
 * Run `action` on the input, then merge the action's output fields back
 * into the original input object. The action must accept exactly `TInput`.
 * Use `pick` inside the action's pipe if the inner handler needs a subset.
 *
 * Example:
 *   augment(pipe(pick("file"), migrate))
 *   // { file, outputPath } → { file, outputPath, content, migrated }
 */
export function augment<
  TInput extends Record<string, unknown>,
  TOutput extends Record<string, unknown>,
  TRefs extends string = never,
>(
  action: Pipeable<TInput, TOutput, TRefs>,
): TypedAction<TInput, TInput & TOutput, TRefs> {
  // Build AST directly — chain inference fails because [TOutput, TInput]
  // doesn't match merge()'s Record<string, unknown>[] with invariance.
  return typedAction({
    kind: "Chain",
    first: {
      kind: "Parallel",
      actions: [action as Action, identity() as Action],
    },
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Merge" } } },
  });
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
 * Constraint: input must be an object (uses augment internally, which
 * relies on parallel + merge).
 *
 * Example:
 *   pipe(tap(pipe(pick("worktreePath", "description"), implement)), createPR)
 */
export function tap<TInput extends Record<string, unknown>, TOutput = any, TRefs extends string = never>(
  action: Pipeable<TInput, TOutput, TRefs>,
): TypedAction<TInput, TInput, TRefs> {
  // Build AST directly — internal plumbing (action → constant → augment)
  // can't go through typed chain/augment with invariant phantom fields.
  // tap: parallel(chain(action, constant({})), identity()) → merge
  return typedAction({
    kind: "Chain",
    first: {
      kind: "Parallel",
      actions: [
        {
          kind: "Chain",
          first: action as Action,
          rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: {} } } },
        },
        { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Identity" } } },
      ],
    },
    rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Merge" } } },
  });
}

// ---------------------------------------------------------------------------
// Range — produce an integer array [start, start+1, ..., end-1]
// ---------------------------------------------------------------------------

export function range(
  start: number,
  end: number,
): TypedAction<never, number[]> {
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
// Option namespace — combinators for Option<T> tagged unions
// ---------------------------------------------------------------------------

// Shared AST fragments for Option desugaring
const TAG_SOME: Action = { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Some" } } };
const TAG_NONE: Action = { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "None" } } };
const EXTRACT_VALUE: Action = { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: "value" } } };
const DROP: Action = { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Drop" } } };
const IDENTITY: Action = { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Identity" } } };

/** Wrap branch cases with ExtractField("value") auto-unwrapping. */
function optionBranch(someCaseBody: Action, noneCaseBody: Action): Action {
  return {
    kind: "Branch",
    cases: {
      Some: { kind: "Chain", first: EXTRACT_VALUE, rest: someCaseBody },
      None: { kind: "Chain", first: EXTRACT_VALUE, rest: noneCaseBody },
    },
  };
}

/**
 * Option namespace. All combinators produce TypedAction AST nodes that
 * desugar to branch + existing builtins, except collect which uses the
 * CollectSome builtin.
 */
export const Option = {
  /**
   * Wrap a value as Some. `T → Option<T>`
   *
   * Equivalent to `tag<OptionDef<T>, "Some">("Some")`.
   */
  some<T>(): TypedAction<T, OptionT<T>> {
    return typedAction(TAG_SOME);
  },

  /**
   * Produce a None. `void → Option<T>`
   *
   * Equivalent to `tag<OptionDef<T>, "None">("None")`.
   */
  none<T>(): TypedAction<void, OptionT<T>> {
    return typedAction(TAG_NONE);
  },

  /**
   * Transform the Some value. `Option<T> → Option<U>`
   *
   * Desugars to: `branch({ Some: pipe(action, tag("Some")), None: tag("None") })`
   */
  map<T, U>(action: Pipeable<T, U>): TypedAction<OptionT<T>, OptionT<U>> {
    return typedAction(optionBranch(
      { kind: "Chain", first: action as Action, rest: TAG_SOME },
      TAG_NONE,
    ));
  },

  /**
   * Monadic bind (flatMap). If Some, pass the value to action which
   * returns Option<U>. If None, stay None. `Option<T> → Option<U>`
   *
   * This is the most fundamental combinator — map, flatten, and filter
   * are all derivable from andThen + constructors.
   *
   * Desugars to: `branch({ Some: action, None: tag("None") })`
   */
  andThen<T, U>(action: Pipeable<T, OptionT<U>>): TypedAction<OptionT<T>, OptionT<U>> {
    return typedAction(optionBranch(
      action as Action,
      TAG_NONE,
    ));
  },

  /**
   * Extract the Some value or produce a default from an action.
   * `Option<T> → T`
   *
   * The defaultAction takes no meaningful input (never) and must produce T.
   * Use `Option.unwrapOr(constant("fallback"))`.
   *
   * The None branch drops its void payload before calling defaultAction,
   * matching Rust's `unwrap_or_else(|| default)` where the closure takes
   * no arguments.
   *
   * Desugars to: `branch({ Some: identity(), None: pipe(drop(), defaultAction) })`
   */
  unwrapOr<T>(defaultAction: Pipeable<never, T>): TypedAction<OptionT<T>, T> {
    return typedAction({
      kind: "Branch",
      cases: {
        Some: { kind: "Chain", first: EXTRACT_VALUE, rest: IDENTITY },
        None: { kind: "Chain", first: EXTRACT_VALUE, rest: { kind: "Chain", first: DROP, rest: defaultAction as Action } },
      },
    });
  },

  /**
   * Unwrap a nested Option. `Option<Option<T>> → Option<T>`
   *
   * Desugars to: `branch({ Some: identity(), None: tag("None") })`
   */
  flatten<T>(): TypedAction<OptionT<OptionT<T>>, OptionT<T>> {
    return typedAction(optionBranch(
      IDENTITY,
      TAG_NONE,
    ));
  },

  /**
   * Conditional keep. If Some, pass value to predicate which returns
   * Option<T> (some() to keep, none() to discard). If None, stay None.
   * `Option<T> → Option<T>`
   *
   * This has the same signature and desugaring as andThen with T=U.
   * Named "filter" for readability when the intent is filtering.
   *
   * Desugars to: `branch({ Some: predicate, None: tag("None") })`
   */
  filter<T>(predicate: Pipeable<T, OptionT<T>>): TypedAction<OptionT<T>, OptionT<T>> {
    return typedAction(optionBranch(
      predicate as Action,
      TAG_NONE,
    ));
  },

  /**
   * Collect Some values from an array, discarding Nones.
   * `Option<T>[] → T[]`
   *
   * This is a builtin handler (CollectSome) — it can't be expressed
   * as a composition of existing AST nodes because it requires
   * array-level filtering logic.
   */
  collect<T>(): TypedAction<OptionT<T>[], T[]> {
    return typedAction({
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "CollectSome" } },
    });
  },

  /**
   * Test if the value is Some. `Option<T> → boolean`
   *
   * Rarely useful — branch on Some/None directly instead.
   *
   * Desugars to: `branch({ Some: pipe(drop(), constant(true)), None: pipe(drop(), constant(false)) })`
   */
  isSome<T>(): TypedAction<OptionT<T>, boolean> {
    const constTrue: Action = { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: true } } };
    const constFalse: Action = { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: false } } };
    return typedAction(optionBranch(
      { kind: "Chain", first: DROP, rest: constTrue },
      { kind: "Chain", first: DROP, rest: constFalse },
    ));
  },

  /**
   * Test if the value is None. `Option<T> → boolean`
   *
   * Rarely useful — branch on Some/None directly instead.
   *
   * Desugars to: `branch({ Some: pipe(drop(), constant(false)), None: pipe(drop(), constant(true)) })`
   */
  isNone<T>(): TypedAction<OptionT<T>, boolean> {
    const constTrue: Action = { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: true } } };
    const constFalse: Action = { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: false } } };
    return typedAction(optionBranch(
      { kind: "Chain", first: DROP, rest: constFalse },
      { kind: "Chain", first: DROP, rest: constTrue },
    ));
  },
} as const;
