import {
  type Option as OptionT,
  type Pipeable,
  type Result as ResultT,
  type TypedAction,
  type UnionMethods,
  toAction,
  typedAction,
  withUnion,
  branch,
} from "./ast.js";
import { chain } from "./chain.js";
import {
  constant,
  drop,
  getIndex,
  identity,
  panic,
  splitFirst,
  splitLast,
  tag,
} from "./builtins.js";
// Lazy: resultMethods and Result are only accessed inside function bodies, not at module init.
import { Result, resultMethods } from "./result.js";
// ---------------------------------------------------------------------------
// Option dispatch table
// ---------------------------------------------------------------------------

export const optionMethods: UnionMethods = {
  map: (action) => Option.map(action),
  andThen: (action) => Option.andThen(action),
  unwrap: () => Option.unwrap(),
  unwrapOr: (action) => Option.unwrapOr(action),
  flatten: () => Option.flatten(),
  filter: (predicate) => Option.filter(predicate),
  collect: () => Option.collect(),
  isSome: () => Option.isSome(),
  isNone: () => Option.isNone(),
  transpose: () => Option.transpose(),
};

// ---------------------------------------------------------------------------
// Option namespace — combinators for Option<T> tagged unions
// ---------------------------------------------------------------------------

/**
 * Option namespace. All combinators produce TypedAction AST nodes that
 * desugar to branch + existing builtins, except collect which uses the
 * CollectSome builtin.
 */
export const Option = {
  /** Tag combinator: wrap value as `Option.Some`. `T → Option<T>` */
  some: tag("Some", "Option"),
  /** Tag combinator: wrap value as `Option.None`. `void → Option<T>` */
  none: tag("None", "Option"),

  /** Transform the Some value. `Option<T> → Option<U>` */
  map<T, U>(action: Pipeable<T, U>): TypedAction<OptionT<T>, OptionT<U>> {
    return withUnion(
      branch({
        Some: chain(toAction(action), toAction(Option.some)),
        None: Option.none,
      }) as TypedAction<OptionT<T>, OptionT<U>>,
      "Option", optionMethods,
    );
  },

  /**
   * Monadic bind (flatMap). If Some, pass the value to action which
   * returns Option<U>. If None, stay None. `Option<T> → Option<U>`
   */
  andThen<T, U>(
    action: Pipeable<T, OptionT<U>>,
  ): TypedAction<OptionT<T>, OptionT<U>> {
    return withUnion(
      branch({
        Some: action,
        None: Option.none,
      }) as TypedAction<OptionT<T>, OptionT<U>>,
      "Option", optionMethods,
    );
  },

  /**
   * Extract the Some value or panic. `Option<T> → T`
   *
   * Exits the Option family — result has no __union.
   * Panics (fatal, not caught by tryCatch) if the value is None.
   */
  unwrap<T>(): TypedAction<OptionT<T>, T> {
    return branch({
      Some: identity(),
      None: panic("called unwrap on None"),
    }) as TypedAction<OptionT<T>, T>;
  },

  /**
   * Extract the Some value or produce a default from an action.
   * `Option<T> → T`
   *
   * Exits the Option family — result has no __union.
   */
  unwrapOr<T>(defaultAction: Pipeable<void, T>): TypedAction<OptionT<T>, T> {
    return branch({
      Some: identity(),
      None: defaultAction,
    }) as TypedAction<OptionT<T>, T>;
  },

  /** Unwrap a nested Option. `Option<Option<T>> → Option<T>` */
  flatten<T>(): TypedAction<OptionT<OptionT<T>>, OptionT<T>> {
    return withUnion(
      branch({
        Some: identity(),
        None: Option.none,
      }) as TypedAction<OptionT<OptionT<T>>, OptionT<T>>,
      "Option", optionMethods,
    );
  },

  /**
   * Conditional keep. If Some, pass value to predicate which returns
   * Option<T>. If None, stay None. `Option<T> → Option<T>`
   */
  filter<T>(
    predicate: Pipeable<T, OptionT<T>>,
  ): TypedAction<OptionT<T>, OptionT<T>> {
    return withUnion(
      branch({
        Some: predicate,
        None: Option.none,
      }) as TypedAction<OptionT<T>, OptionT<T>>,
      "Option", optionMethods,
    );
  },

  /**
   * Collect Some values from an array, discarding Nones.
   * `Option<T>[] → T[]`
   *
   * Exits the Option family — result is T[], not Option.
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
   * Exits the Option family — result is boolean, not Option.
   */
  isSome<T>(): TypedAction<OptionT<T>, boolean> {
    return branch({
      Some: constant(true),
      None: constant(false),
    }) as TypedAction<OptionT<T>, boolean>;
  },

  /**
   * Test if the value is None. `Option<T> → boolean`
   *
   * Exits the Option family — result is boolean, not Option.
   */
  isNone<T>(): TypedAction<OptionT<T>, boolean> {
    return branch({
      Some: constant(false),
      None: constant(true),
    }) as TypedAction<OptionT<T>, boolean>;
  },

  /**
   * Swap Option/Result nesting.
   * `Option<Result<TValue, TError>> → Result<Option<TValue>, TError>`
   *
   * - Some(Ok(t))  → Ok(Some(t))
   * - Some(Err(e)) → Err(e)
   * - None         → Ok(None)
   *
   * Changes family — result carries resultMethods.
   */
  transpose<TValue, TError>(): TypedAction<
    OptionT<ResultT<TValue, TError>>,
    ResultT<OptionT<TValue>, TError>
  > {
    return withUnion(
      branch({
        Some: branch({
          Ok: chain(toAction(Option.some), toAction(Result.ok)),
          Err: Result.err,
        }),
        None: chain(toAction(chain(toAction(drop), toAction(Option.none))), toAction(Result.ok)),
      }) as TypedAction<
        OptionT<ResultT<TValue, TError>>,
        ResultT<OptionT<TValue>, TError>
      >,
      "Result", resultMethods,
    );
  },

} as const;

// ---------------------------------------------------------------------------
// First — extract the first element of an array as Option<TElement>
// ---------------------------------------------------------------------------

/**
 * Extract the first element of an array.
 * `readonly TElement[] → Option<TElement>`
 *
 * Composes `splitFirst` (which returns `Option<[TElement, TElement[]]>`)
 * with `Option.map(getIndex(0))` to extract just the element.
 *
 * Output carries optionMethods via chain propagation from Option.map.
 */
export function first<TElement>(): TypedAction<
  readonly TElement[],
  OptionT<TElement>
> {
  return chain(
    toAction(splitFirst()),
    toAction(Option.map(toAction(getIndex(0).unwrap()))),
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
 *
 * Output carries optionMethods via chain propagation from Option.map.
 */
export function last<TElement>(): TypedAction<
  readonly TElement[],
  OptionT<TElement>
> {
  return chain(
    toAction(splitLast()),
    toAction(Option.map(toAction(getIndex(1).unwrap()))),
  ) as TypedAction<readonly TElement[], OptionT<TElement>>;
}
