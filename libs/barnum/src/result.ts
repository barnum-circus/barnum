import {
  type Option as OptionT,
  type Pipeable,
  type Result as ResultT,
  type ResultDef,
  type TypedAction,
  toAction,
  branch,
} from "./ast.js";
import { chain } from "./chain.js";
import { constant, drop, identity, panic, tag } from "./builtins/index.js";
import { Option } from "./option.js";

// ---------------------------------------------------------------------------
// Result namespace — combinators for Result<TValue, TError> tagged unions
// ---------------------------------------------------------------------------

export const Result = {
  /** Tag combinator: wrap value as `Result.Ok`. `TValue → Result<TValue, TError>` */
  ok<TValue, TError = never>(): TypedAction<TValue, ResultT<TValue, TError>> {
    return tag<"Result", ResultDef<TValue, TError>, "Ok">("Ok", "Result");
  },
  /** Tag combinator: wrap value as `Result.Err`. `TError → Result<TValue, TError>` */
  err<TValue = never, TError = unknown>(): TypedAction<TError, ResultT<TValue, TError>> {
    return tag<"Result", ResultDef<TValue, TError>, "Err">("Err", "Result");
  },

  /** Transform the Ok value. `Result<TValue, TError> → Result<TOut, TError>` */
  map<TValue, TOut, TError>(
    action: Pipeable<TValue, TOut>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return branch({
      Ok: chain(toAction(action), toAction(Result.ok())),
      Err: Result.err(),
    }) as TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>>;
  },

  /** Transform the Err value. `Result<TValue, TError> → Result<TValue, TErrorOut>` */
  mapErr<TValue, TError, TErrorOut>(
    action: Pipeable<TError, TErrorOut>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>> {
    return branch({
      Ok: Result.ok(),
      Err: chain(toAction(action), toAction(Result.err())),
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
      Err: Result.err(),
    }) as TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>>;
  },

  /** Fallback on Err. If Ok, keep it. If Err, pass error to fallback. */
  or<TValue, TError, TErrorOut>(
    fallback: Pipeable<TError, ResultT<TValue, TErrorOut>>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>> {
    return branch({
      Ok: Result.ok(),
      Err: fallback,
    }) as TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>>;
  },


  /**
   * Extract the Ok value or panic. `Result<TValue, TError> → TValue`
   *
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
   */
  toOption<TValue, TError>(): TypedAction<
    ResultT<TValue, TError>,
    OptionT<TValue>
  > {
    return branch({
      Ok: Option.some(),
      Err: chain(toAction(drop), toAction(Option.none())),
    }) as TypedAction<ResultT<TValue, TError>, OptionT<TValue>>;
  },

  /**
   * Convert Err to Some, Ok to None. `Result<TValue, TError> → Option<TError>`
   */
  toOptionErr<TValue, TError>(): TypedAction<
    ResultT<TValue, TError>,
    OptionT<TError>
  > {
    return branch({
      Ok: chain(toAction(drop), toAction(Option.none())),
      Err: Option.some(),
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
        Some: chain(toAction(Result.ok()), toAction(Option.some())),
        None: chain(toAction(drop), toAction(Option.none())),
      }),
      Err: chain(toAction(Result.err()), toAction(Option.some())),
    }) as TypedAction<
      ResultT<OptionT<TValue>, TError>,
      OptionT<ResultT<TValue, TError>>
    >;
  },

  /**
   * Test if the value is Ok. `Result<TValue, TError> → boolean`
   */
  isOk<TValue, TError>(): TypedAction<ResultT<TValue, TError>, boolean> {
    return branch({
      Ok: constant(true),
      Err: constant(false),
    }) as TypedAction<ResultT<TValue, TError>, boolean>;
  },

  /**
   * Test if the value is Err. `Result<TValue, TError> → boolean`
   */
  isErr<TValue, TError>(): TypedAction<ResultT<TValue, TError>, boolean> {
    return branch({
      Ok: constant(false),
      Err: constant(true),
    }) as TypedAction<ResultT<TValue, TError>, boolean>;
  },

} as const;
