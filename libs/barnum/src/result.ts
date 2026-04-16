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
import { optionMethods } from "./option.js";
import { z } from "zod";

// ---------------------------------------------------------------------------
// Result dispatch table
// ---------------------------------------------------------------------------

export const resultMethods: UnionMethods = {
  map: (action) => Result.map(action),
  andThen: (action) => Result.andThen(action),
  unwrap: () => Result.unwrap(),
  unwrapOr: (action) => Result.unwrapOr(action),
  flatten: () => Result.flatten(),
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
  /** Wrap a value as Ok. `TValue → Result<TValue, TError>` */
  ok<TValue, TError>(): TypedAction<TValue, ResultT<TValue, TError>> {
    return withUnion(
      tag("Ok") as TypedAction<TValue, ResultT<TValue, TError>>,
      "Result", resultMethods,
    );
  },

  /** Wrap a value as Err. `TError → Result<TValue, TError>` */
  err<TValue, TError>(): TypedAction<TError, ResultT<TValue, TError>> {
    return withUnion(
      tag("Err") as TypedAction<TError, ResultT<TValue, TError>>,
      "Result", resultMethods,
    );
  },

  /** Transform the Ok value. `Result<TValue, TError> → Result<TOut, TError>` */
  map<TValue, TOut, TError>(
    action: Pipeable<TValue, TOut>,
  ): TypedAction<ResultT<TValue, TError>, ResultT<TOut, TError>> {
    return withUnion(
      branch({
        Ok: chain(toAction(action), toAction(tag("Ok"))),
        Err: tag("Err"),
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
        Ok: tag("Ok"),
        Err: chain(toAction(action), toAction(tag("Err"))),
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
        Err: tag("Err"),
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
        Ok: tag("Ok"),
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
        Err: tag("Err"),
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

  /** Unwrap nested Result. `Result<Result<TValue, TError>, TError> → Result<TValue, TError>` */
  flatten<TValue, TError>(): TypedAction<
    ResultT<ResultT<TValue, TError>, TError>,
    ResultT<TValue, TError>
  > {
    return withUnion(
      branch({
        Ok: identity(),
        Err: tag("Err"),
      }) as TypedAction<
        ResultT<ResultT<TValue, TError>, TError>,
        ResultT<TValue, TError>
      >,
      "Result", resultMethods,
    );
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
        Ok: tag("Some"),
        Err: drop.tag("None"),
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
        Ok: drop.tag("None"),
        Err: tag("Some"),
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
          Some: chain(toAction(tag("Ok")), toAction(tag("Some"))),
          None: drop.tag("None"),
        }),
        Err: chain(toAction(tag("Err")), toAction(tag("Some"))),
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
