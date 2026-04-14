import {
  type Option as OptionT,
  type Pipeable,
  type Result as ResultT,
  type TypedAction,
  branch,
} from "./ast.js";
import { chain } from "./chain.js";
import { constant, drop, identity, tag } from "./builtins.js";
import { z } from "zod";

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
    other: Pipeable<void, ResultT<TOut, TError>>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return branch({
      Ok: chain(drop, other),
      Err: tag("Err"),
    }) as TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>>;
  },

  /**
   * Extract Ok or compute default from Err. `Result<TValue, TError> → TValue`
   *
   * With covariant output, throw tokens (Out=never) are assignable to
   * Pipeable<TError, TValue>. For inference-free usage with throw tokens,
   * prefer the postfix method: `handler.unwrapOr(throwError)`.
   */
  unwrapOr<TValue, TError>(
    defaultAction: Pipeable<TError, TValue>,
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
      Ok: constant(true),
      Err: constant(false),
    }) as TypedAction<ResultT<TValue, TError>, boolean>;
  },

  /** Test if the value is Err. `Result<TValue, TError> → boolean` */
  isErr<TValue, TError>(): TypedAction<ResultT<TValue, TError>, boolean> {
    return branch({
      Ok: constant(false),
      Err: constant(true),
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
