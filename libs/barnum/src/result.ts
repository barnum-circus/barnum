import {
  type Option as OptionT,
  type Pipeable,
  type Result as ResultT,
  type ResultDef,
  type TypedAction,
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
  err<TValue = never, TError = unknown>(): TypedAction<
    TError,
    ResultT<TValue, TError>
  > {
    return tag<"Result", ResultDef<TValue, TError>, "Err">("Err", "Result");
  },

  /** Transform the Ok value. `Result<TValue, TError> → Result<TOut, TError>` */
  map<TValue, TOut, TError>(
    action: Pipeable<TValue, TOut>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return branch({
      Ok: chain(action, Result.ok<TOut, TError>()),
      Err: Result.err<TOut, TError>(),
    }) as TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>>;
  },

  /** Transform the Err value. `Result<TValue, TError> → Result<TValue, TErrorOut>` */
  mapErr<TValue, TError, TErrorOut>(
    action: Pipeable<TError, TErrorOut>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>> {
    return branch({
      Ok: Result.ok<TValue, TErrorOut>(),
      Err: chain(action, Result.err<TValue, TErrorOut>()),
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
      Err: Result.err<TOut, TError>(),
    }) as TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>>;
  },

  /** Fallback on Err. If Ok, keep it. If Err, pass error to fallback. */
  or<TValue, TError, TErrorOut>(
    fallback: Pipeable<TError, ResultT<TValue, TErrorOut>>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TValue, TErrorOut>> {
    return branch({
      Ok: Result.ok<TValue, TErrorOut>(),
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
      Ok: identity<TValue>(),
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
      Ok: identity<TValue>(),
      Err: defaultAction,
    }) as TypedAction<ResultT<TValue, TError>, TValue>;
  },

  /**
   * Convert Ok to Some, Err to None. `Result<TValue, TError> → Option<TValue>`
   */
  asOkOption<TValue, TError>(): TypedAction<
    ResultT<TValue, TError>,
    OptionT<TValue>
  > {
    return branch({
      Ok: Option.some<TValue>(),
      Err: chain(drop, Option.none<TValue>()),
    }) as TypedAction<ResultT<TValue, TError>, OptionT<TValue>>;
  },

  /**
   * Convert Err to Some, Ok to None. `Result<TValue, TError> → Option<TError>`
   */
  asErrOption<TValue, TError>(): TypedAction<
    ResultT<TValue, TError>,
    OptionT<TError>
  > {
    return branch({
      Ok: chain(drop, Option.none<TError>()),
      Err: Option.some<TError>(),
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
        Some: chain(
          Result.ok<TValue, TError>(),
          Option.some<ResultT<TValue, TError>>(),
        ),
        None: chain(drop, Option.none<ResultT<TValue, TError>>()),
      }),
      Err: chain(
        Result.err<TValue, TError>(),
        Option.some<ResultT<TValue, TError>>(),
      ),
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
      Ok: constant<boolean>(true),
      Err: constant<boolean>(false),
    }) as TypedAction<ResultT<TValue, TError>, boolean>;
  },

  /**
   * Test if the value is Err. `Result<TValue, TError> → boolean`
   */
  isErr<TValue, TError>(): TypedAction<ResultT<TValue, TError>, boolean> {
    return branch({
      Ok: constant<boolean>(false),
      Err: constant<boolean>(true),
    }) as TypedAction<ResultT<TValue, TError>, boolean>;
  },
} as const;
