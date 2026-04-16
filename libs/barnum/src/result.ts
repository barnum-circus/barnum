import {
  type Option as OptionT,
  type Pipeable,
  type Result as ResultT,
  type TypedAction,
  type UnionMethods,
  toAction,
  withUnion,
  branch,
} from "./ast.js";
import { chain } from "./chain.js";
import { constant, drop, identity, panic, tag } from "./builtins.js";
import { Option, optionMethods } from "./option.js";
// ---------------------------------------------------------------------------
// Result dispatch table
// ---------------------------------------------------------------------------

export const resultMethods: UnionMethods = {
  map: (action) => Result.map(action),
  andThen: (action) => Result.andThen(action),
  unwrap: () => Result.unwrap(),
  unwrapOr: (action) => Result.unwrapOr(action),
  mapErr: (action) => Result.mapErr(action),
  and: (other) => Result.and(other),
  or: (fallback) => Result.or(fallback),
  toOption: () => Result.toOption(),
  toOptionErr: () => Result.toOptionErr(),
  transpose: () => Result.transpose(),
  isOk: () => Result.isOk(),
  isErr: () => Result.isErr(),
};

// ---------------------------------------------------------------------------
// Result namespace — combinators for Result<TValue, TError> tagged unions
// ---------------------------------------------------------------------------

export const Result = {
  /** Tag combinator: wrap value as `Result.Ok`. `TValue → Result<TValue, TError>` */
  ok: tag("Ok", "Result"),
  /** Tag combinator: wrap value as `Result.Err`. `TError → Result<TValue, TError>` */
  err: tag("Err", "Result"),

  /** Transform the Ok value. `Result<TValue, TError> → Result<TOut, TError>` */
  map<TValue, TOut, TError>(
    action: Pipeable<TValue, TOut>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return withUnion(
      branch({
        Ok: chain(toAction(action), toAction(Result.ok)),
        Err: Result.err,
      }) as TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>>,
      "Result", resultMethods,
    );
  },

  /** Transform the Err value. `Result<TValue, TError> → Result<TValue, TErrorOut>` */
  mapErr<TValue, TError, TErrorOut>(
    action: Pipeable<TError, TErrorOut>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>> {
    return withUnion(
      branch({
        Ok: Result.ok,
        Err: chain(toAction(action), toAction(Result.err)),
      }) as TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>>,
      "Result", resultMethods,
    );
  },

  /**
   * Monadic bind (flatMap) for Ok. If Ok, pass value to action which
   * returns Result<TOut, TError>. If Err, propagate.
   */
  andThen<TValue, TOut, TError>(
    action: Pipeable<TValue, ResultT<TOut, TError>>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return withUnion(
      branch({
        Ok: action,
        Err: Result.err,
      }) as TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>>,
      "Result", resultMethods,
    );
  },

  /** Fallback on Err. If Ok, keep it. If Err, pass error to fallback. */
  or<TValue, TError, TErrorOut>(
    fallback: Pipeable<TError, ResultT<TValue, TErrorOut>>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>> {
    return withUnion(
      branch({
        Ok: Result.ok,
        Err: fallback,
      }) as TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>>,
      "Result", resultMethods,
    );
  },

  /** Replace Ok value with another Result. If Ok, discard value and return other. */
  and<TValue, TOut, TError>(
    other: Pipeable<void, ResultT<TOut, TError>>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return withUnion(
      branch({
        Ok: chain(drop, other),
        Err: Result.err,
      }) as TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>>,
      "Result", resultMethods,
    );
  },

  /**
   * Extract the Ok value or panic. `Result<TValue, TError> → TValue`
   *
   * Exits the Result family — result has no __union.
   * Panics (fatal, not caught by tryCatch) if the value is Err.
   */
  unwrap<TValue, TError>(): TypedAction<ResultT<TValue, TError>, TValue> {
    return branch({
      Ok: identity(),
      Err: panic("called unwrap on Err"),
    }) as TypedAction<ResultT<TValue, TError>, TValue>;
  },

  /**
   * Extract Ok or compute default from Err. `Result<TValue, TError> → TValue`
   *
   * Exits the Result family — result has no __union.
   */
  unwrapOr<TValue, TError>(
    defaultAction: Pipeable<TError, TValue>,
  ): TypedAction<ResultT<TValue, TError>, TValue> {
    return branch({
      Ok: identity(),
      Err: defaultAction,
    }) as TypedAction<ResultT<TValue, TError>, TValue>;
  },

  /**
   * Convert Ok to Some, Err to None. `Result<TValue, TError> → Option<TValue>`
   *
   * Changes family — result carries optionMethods.
   */
  toOption<TValue, TError>(): TypedAction<
    ResultT<TValue, TError>,
    OptionT<TValue>
  > {
    return withUnion(
      branch({
        Ok: Option.some,
        Err: chain(toAction(drop), toAction(Option.none)),
      }) as TypedAction<ResultT<TValue, TError>, OptionT<TValue>>,
      "Option", optionMethods,
    );
  },

  /**
   * Convert Err to Some, Ok to None. `Result<TValue, TError> → Option<TError>`
   *
   * Changes family — result carries optionMethods.
   */
  toOptionErr<TValue, TError>(): TypedAction<
    ResultT<TValue, TError>,
    OptionT<TError>
  > {
    return withUnion(
      branch({
        Ok: chain(toAction(drop), toAction(Option.none)),
        Err: Option.some,
      }) as TypedAction<ResultT<TValue, TError>, OptionT<TError>>,
      "Option", optionMethods,
    );
  },

  /**
   * Swap Result/Option nesting.
   * `Result<Option<TValue>, TError> → Option<Result<TValue, TError>>`
   *
   * Changes family — result carries optionMethods.
   */
  transpose<TValue, TError>(): TypedAction<
    ResultT<OptionT<TValue>, TError>,
    OptionT<ResultT<TValue, TError>>
  > {
    return withUnion(
      branch({
        Ok: branch({
          Some: chain(toAction(Result.ok), toAction(Option.some)),
          None: chain(toAction(drop), toAction(Option.none)),
        }),
        Err: chain(toAction(Result.err), toAction(Option.some)),
      }) as TypedAction<
        ResultT<OptionT<TValue>, TError>,
        OptionT<ResultT<TValue, TError>>
      >,
      "Option", optionMethods,
    );
  },

  /**
   * Test if the value is Ok. `Result<TValue, TError> → boolean`
   *
   * Exits the Result family — result is boolean, not Result.
   */
  isOk<TValue, TError>(): TypedAction<ResultT<TValue, TError>, boolean> {
    return branch({
      Ok: constant(true),
      Err: constant(false),
    }) as TypedAction<ResultT<TValue, TError>, boolean>;
  },

  /**
   * Test if the value is Err. `Result<TValue, TError> → boolean`
   *
   * Exits the Result family — result is boolean, not Result.
   */
  isErr<TValue, TError>(): TypedAction<ResultT<TValue, TError>, boolean> {
    return branch({
      Ok: constant(false),
      Err: constant(true),
    }) as TypedAction<ResultT<TValue, TError>, boolean>;
  },

} as const;
