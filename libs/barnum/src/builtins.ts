import {
  type Action,
  type Option as OptionT,
  type Pipeable,
  type Result as ResultT,
  type TaggedUnion,
  type TypedAction,
  typedAction,
} from "./ast.js";
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

export function constant<TValue>(value: TValue): TypedAction<any, TValue> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Constant", value } },
  });
}

// ---------------------------------------------------------------------------
// Identity — pass input through unchanged
// ---------------------------------------------------------------------------

export const identity: TypedAction<any, any> = typedAction({
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Identity" } },
});

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
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Tag", value: kind } },
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

export function extractIndex<TTuple extends unknown[], TIndex extends number>(
  index: TIndex,
): TypedAction<TTuple, TTuple[TIndex]> {
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

export function dropResult<TInput, TOutput>(
  action: Pipeable<TInput, TOutput>,
): TypedAction<TInput, never> {
  // Build AST directly — chain inference fails when drop's TValue
  // isn't constrained by context (resolves to unknown ≠ TOutput).
  return typedAction({
    kind: "Chain",
    first: action as Action,
    rest: {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Drop" } },
    },
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

  // Step 1: all(create, identity) → [TResource, TIn] → merge → TResource & TIn
  const acquireAndMerge = chain(
    typedAction<TIn, [TResource, TIn]>({
      kind: "All",
      actions: [create as Action, identity as Action],
    }),
    typedAction<[TResource, TIn], TResource & TIn>(mergeBuiltin),
  );

  // Step 2: all(action, identity) → [TOut, TResource & TIn]
  // Keep merged object so dispose can access resource fields.
  const actionAndKeepMerged = typedAction<
    TResource & TIn,
    [TOut, TResource & TIn]
  >({
    kind: "All",
    actions: [action as Action, identity as Action],
  });

  // Step 3: all(extractIndex(0), chain(extractIndex(1), dispose)) → [TOut, unknown]
  const disposeAndKeepResult = typedAction<
    [TOut, TResource & TIn],
    [TOut, unknown]
  >({
    kind: "All",
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
>(
  action: Pipeable<TInput, TOutput>,
): TypedAction<TInput, TInput & TOutput> {
  // Build AST directly — chain inference fails because [TOutput, TInput]
  // doesn't match merge()'s Record<string, unknown>[] with invariance.
  return typedAction({
    kind: "Chain",
    first: {
      kind: "All",
      actions: [action as Action, identity as Action],
    },
    rest: {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Merge" } },
    },
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
 * relies on all + merge).
 *
 * Example:
 *   pipe(tap(pipe(pick("worktreePath", "description"), implement)), createPR)
 */
export function tap<
  TInput extends Record<string, unknown>,
>(action: Pipeable<TInput, any>): TypedAction<TInput, TInput> {
  // Build AST directly — internal plumbing (action → constant → augment)
  // can't go through typed chain/augment with invariant phantom fields.
  // tap: all(chain(action, constant({})), identity()) → merge
  return typedAction({
    kind: "Chain",
    first: {
      kind: "All",
      actions: [
        {
          kind: "Chain",
          first: action as Action,
          rest: {
            kind: "Invoke",
            handler: {
              kind: "Builtin",
              builtin: { kind: "Constant", value: {} },
            },
          },
        },
        {
          kind: "Invoke",
          handler: { kind: "Builtin", builtin: { kind: "Identity" } },
        },
      ],
    },
    rest: {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Merge" } },
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
// Option namespace — combinators for Option<T> tagged unions
// ---------------------------------------------------------------------------

// Shared AST fragments for Option desugaring
const TAG_SOME: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Some" } },
};
const TAG_NONE: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "None" } },
};
const EXTRACT_VALUE: Action = {
  kind: "Invoke",
  handler: {
    kind: "Builtin",
    builtin: { kind: "ExtractField", value: "value" },
  },
};
const DROP: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Drop" } },
};
const IDENTITY: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Identity" } },
};

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
   * Produce a None. `never → Option<T>`
   *
   * Chain after `.drop()` to discard the current value first.
   * Equivalent to `tag<OptionDef<T>, "None">("None")`.
   */
  none<T>(): TypedAction<never, OptionT<T>> {
    return typedAction(TAG_NONE);
  },

  /**
   * Transform the Some value. `Option<T> → Option<U>`
   *
   * Desugars to: `branch({ Some: pipe(action, tag("Some")), None: tag("None") })`
   */
  map<T, U>(action: Pipeable<T, U>): TypedAction<OptionT<T>, OptionT<U>> {
    return typedAction(
      optionBranch(
        { kind: "Chain", first: action as Action, rest: TAG_SOME },
        TAG_NONE,
      ),
    );
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
  andThen<T, U>(
    action: Pipeable<T, OptionT<U>>,
  ): TypedAction<OptionT<T>, OptionT<U>> {
    return typedAction(optionBranch(action as Action, TAG_NONE));
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
        None: {
          kind: "Chain",
          first: EXTRACT_VALUE,
          rest: { kind: "Chain", first: DROP, rest: defaultAction as Action },
        },
      },
    });
  },

  /**
   * Unwrap a nested Option. `Option<Option<T>> → Option<T>`
   *
   * Desugars to: `branch({ Some: identity(), None: tag("None") })`
   */
  flatten<T>(): TypedAction<OptionT<OptionT<T>>, OptionT<T>> {
    return typedAction(optionBranch(IDENTITY, TAG_NONE));
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
  filter<T>(
    predicate: Pipeable<T, OptionT<T>>,
  ): TypedAction<OptionT<T>, OptionT<T>> {
    return typedAction(optionBranch(predicate as Action, TAG_NONE));
  },

  /**
   * Collect Some values from an array, discarding Nones.
   * `Option<T>[] → T[]`
   *
   * This is a builtin handler (CollectSome) — it can't be expressed
   * as a composition of existing AST nodes because it requires
   * array-level filtering logic.
   */
  collect<T = any>(): TypedAction<OptionT<T>[], T[]> {
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
    const constTrue: Action = {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Constant", value: true } },
    };
    const constFalse: Action = {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Constant", value: false } },
    };
    return typedAction(
      optionBranch(
        { kind: "Chain", first: DROP, rest: constTrue },
        { kind: "Chain", first: DROP, rest: constFalse },
      ),
    );
  },

  /**
   * Test if the value is None. `Option<T> → boolean`
   *
   * Rarely useful — branch on Some/None directly instead.
   *
   * Desugars to: `branch({ Some: pipe(drop(), constant(false)), None: pipe(drop(), constant(true)) })`
   */
  isNone<T>(): TypedAction<OptionT<T>, boolean> {
    const constTrue: Action = {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Constant", value: true } },
    };
    const constFalse: Action = {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Constant", value: false } },
    };
    return typedAction(
      optionBranch(
        { kind: "Chain", first: DROP, rest: constFalse },
        { kind: "Chain", first: DROP, rest: constTrue },
      ),
    );
  },
} as const;

// ---------------------------------------------------------------------------
// Result namespace — combinators for Result<TValue, TError> tagged unions
// ---------------------------------------------------------------------------

// Shared AST fragments for Result desugaring
const TAG_OK: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Ok" } },
};
const TAG_ERR: Action = {
  kind: "Invoke",
  handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Err" } },
};

/** Wrap branch cases with ExtractField("value") auto-unwrapping. */
function resultBranch(okCaseBody: Action, errCaseBody: Action): Action {
  return {
    kind: "Branch",
    cases: {
      Ok: { kind: "Chain", first: EXTRACT_VALUE, rest: okCaseBody },
      Err: { kind: "Chain", first: EXTRACT_VALUE, rest: errCaseBody },
    },
  };
}

/**
 * Result namespace. All combinators produce TypedAction AST nodes that
 * desugar to branch + existing builtins.
 */
export const Result = {
  /**
   * Wrap a value as Ok. `TValue → Result<TValue, TError>`
   */
  ok<TValue, TError>(): TypedAction<TValue, ResultT<TValue, TError>> {
    return typedAction(TAG_OK);
  },

  /**
   * Wrap a value as Err. `TError → Result<TValue, TError>`
   */
  err<TValue, TError>(): TypedAction<TError, ResultT<TValue, TError>> {
    return typedAction(TAG_ERR);
  },

  /**
   * Transform the Ok value. `Result<TValue, TError> → Result<TOut, TError>`
   *
   * Desugars to: `branch({ Ok: pipe(action, tag("Ok")), Err: tag("Err") })`
   */
  map<TValue, TOut, TError>(
    action: Pipeable<TValue, TOut>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return typedAction(
      resultBranch(
        { kind: "Chain", first: action as Action, rest: TAG_OK },
        TAG_ERR,
      ),
    );
  },

  /**
   * Transform the Err value. `Result<TValue, TError> → Result<TValue, TErrorOut>`
   *
   * Desugars to: `branch({ Ok: tag("Ok"), Err: pipe(action, tag("Err")) })`
   */
  mapErr<TValue, TError, TErrorOut>(
    action: Pipeable<TError, TErrorOut>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>> {
    return typedAction(
      resultBranch(TAG_OK, {
        kind: "Chain",
        first: action as Action,
        rest: TAG_ERR,
      }),
    );
  },

  /**
   * Monadic bind (flatMap) for Ok. If Ok, pass value to action which
   * returns Result<TOut, TError>. If Err, propagate.
   *
   * Desugars to: `branch({ Ok: action, Err: tag("Err") })`
   */
  andThen<TValue, TOut, TError>(
    action: Pipeable<TValue, ResultT<TOut, TError>>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return typedAction(resultBranch(action as Action, TAG_ERR));
  },

  /**
   * Fallback on Err. If Ok, keep it. If Err, pass error to fallback
   * which returns a new Result.
   *
   * Desugars to: `branch({ Ok: tag("Ok"), Err: fallback })`
   */
  or<TValue, TError, TErrorOut>(
    fallback: Pipeable<TError, ResultT<TValue, TErrorOut>>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>> {
    return typedAction(resultBranch(TAG_OK, fallback as Action));
  },

  /**
   * Replace Ok value with another Result. If Ok, discard value and
   * return other. If Err, propagate.
   *
   * Desugars to: `branch({ Ok: pipe(drop(), other), Err: tag("Err") })`
   */
  and<TValue, TOut, TError>(
    other: Pipeable<never, ResultT<TOut, TError>>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return typedAction(
      resultBranch(
        { kind: "Chain", first: DROP, rest: other as Action },
        TAG_ERR,
      ),
    );
  },

  /**
   * Extract Ok or compute default from Err. `Result<TValue, TError> → TValue`
   *
   * Takes an action that receives the Err payload and produces a fallback.
   * Uses covariant output checking so throw tokens (Out=never) are assignable
   * when TValue is provided explicitly: `Result.unwrapOr<string, string>(throwError)`.
   *
   * For inference-free usage with throw tokens, prefer the postfix method:
   * `handler.unwrapOr(throwError)` — the `this` constraint provides TValue.
   *
   * Desugars to: `branch({ Ok: identity(), Err: defaultAction })`
   */
  unwrapOr<TValue, TError>(
    defaultAction: Action & {
      __in?: (input: TError) => void;
      __out?: () => TValue;
    },
  ): TypedAction<ResultT<TValue, TError>, TValue> {
    return typedAction(resultBranch(IDENTITY, defaultAction as Action));
  },

  /**
   * Unwrap nested Result. `Result<Result<TValue, TError>, TError> → Result<TValue, TError>`
   *
   * Desugars to: `branch({ Ok: identity(), Err: tag("Err") })`
   */
  flatten<TValue, TError>(): TypedAction<
    ResultT<ResultT<TValue, TError>, TError>,
    ResultT<TValue, TError>
  > {
    return typedAction(resultBranch(IDENTITY, TAG_ERR));
  },

  /**
   * Convert Ok to Some, Err to None. `Result<TValue, TError> → Option<TValue>`
   *
   * Desugars to: `branch({ Ok: tag("Some"), Err: pipe(drop(), tag("None")) })`
   */
  toOption<TValue, TError>(): TypedAction<
    ResultT<TValue, TError>,
    OptionT<TValue>
  > {
    return typedAction(
      resultBranch(TAG_SOME, { kind: "Chain", first: DROP, rest: TAG_NONE }),
    );
  },

  /**
   * Convert Err to Some, Ok to None. `Result<TValue, TError> → Option<TError>`
   *
   * Desugars to: `branch({ Ok: pipe(drop(), tag("None")), Err: tag("Some") })`
   */
  toOptionErr<TValue, TError>(): TypedAction<
    ResultT<TValue, TError>,
    OptionT<TError>
  > {
    return typedAction(
      resultBranch({ kind: "Chain", first: DROP, rest: TAG_NONE }, TAG_SOME),
    );
  },

  /**
   * Swap Result/Option nesting.
   * `Result<Option<TValue>, TError> → Option<Result<TValue, TError>>`
   */
  transpose<TValue, TError>(): TypedAction<
    ResultT<OptionT<TValue>, TError>,
    OptionT<ResultT<TValue, TError>>
  > {
    return typedAction(
      resultBranch(
        // Ok case: receives Option<TValue>, branch on Some/None
        {
          kind: "Branch",
          cases: {
            Some: {
              kind: "Chain",
              first: EXTRACT_VALUE,
              rest: { kind: "Chain", first: TAG_OK, rest: TAG_SOME },
            },
            None: {
              kind: "Chain",
              first: EXTRACT_VALUE,
              rest: { kind: "Chain", first: DROP, rest: TAG_NONE },
            },
          },
        },
        // Err case: receives TError, wrap as Result.err then Option.some
        { kind: "Chain", first: TAG_ERR, rest: TAG_SOME },
      ),
    );
  },

  /**
   * Test if the value is Ok. `Result<TValue, TError> → boolean`
   */
  isOk<TValue, TError>(): TypedAction<ResultT<TValue, TError>, boolean> {
    const constTrue: Action = {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Constant", value: true } },
    };
    const constFalse: Action = {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Constant", value: false } },
    };
    return typedAction(
      resultBranch(
        { kind: "Chain", first: DROP, rest: constTrue },
        { kind: "Chain", first: DROP, rest: constFalse },
      ),
    );
  },

  /**
   * Test if the value is Err. `Result<TValue, TError> → boolean`
   */
  isErr<TValue, TError>(): TypedAction<ResultT<TValue, TError>, boolean> {
    const constTrue: Action = {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Constant", value: true } },
    };
    const constFalse: Action = {
      kind: "Invoke",
      handler: { kind: "Builtin", builtin: { kind: "Constant", value: false } },
    };
    return typedAction(
      resultBranch(
        { kind: "Chain", first: DROP, rest: constFalse },
        { kind: "Chain", first: DROP, rest: constTrue },
      ),
    );
  },
} as const;
