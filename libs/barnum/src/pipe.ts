import type { Action, TypedAction } from "./ast.js";
import { typedAction } from "./ast.js";
import { identity } from "./builtins.js";
import { chain } from "./chain.js";

export function pipe<T1, T2, R1 extends string>(
  a1: TypedAction<T1, T2, R1>,
): TypedAction<T1, T2, R1>;
export function pipe<T1, T2, T3, R1 extends string, R2 extends string>(
  a1: TypedAction<T1, T2, R1>,
  a2: TypedAction<T2, T3, R2>,
): TypedAction<T1, T3, R1 | R2>;
export function pipe<
  T1, T2, T3, T4,
  R1 extends string, R2 extends string, R3 extends string,
>(
  a1: TypedAction<T1, T2, R1>,
  a2: TypedAction<T2, T3, R2>,
  a3: TypedAction<T3, T4, R3>,
): TypedAction<T1, T4, R1 | R2 | R3>;
export function pipe<
  T1, T2, T3, T4, T5,
  R1 extends string, R2 extends string, R3 extends string, R4 extends string,
>(
  a1: TypedAction<T1, T2, R1>,
  a2: TypedAction<T2, T3, R2>,
  a3: TypedAction<T3, T4, R3>,
  a4: TypedAction<T4, T5, R4>,
): TypedAction<T1, T5, R1 | R2 | R3 | R4>;
export function pipe<
  T1, T2, T3, T4, T5, T6,
  R1 extends string, R2 extends string, R3 extends string,
  R4 extends string, R5 extends string,
>(
  a1: TypedAction<T1, T2, R1>,
  a2: TypedAction<T2, T3, R2>,
  a3: TypedAction<T3, T4, R3>,
  a4: TypedAction<T4, T5, R4>,
  a5: TypedAction<T5, T6, R5>,
): TypedAction<T1, T6, R1 | R2 | R3 | R4 | R5>;
export function pipe<
  T1, T2, T3, T4, T5, T6, T7,
  R1 extends string, R2 extends string, R3 extends string,
  R4 extends string, R5 extends string, R6 extends string,
>(
  a1: TypedAction<T1, T2, R1>,
  a2: TypedAction<T2, T3, R2>,
  a3: TypedAction<T3, T4, R3>,
  a4: TypedAction<T4, T5, R4>,
  a5: TypedAction<T5, T6, R5>,
  a6: TypedAction<T6, T7, R6>,
): TypedAction<T1, T7, R1 | R2 | R3 | R4 | R5 | R6>;
export function pipe<
  T1, T2, T3, T4, T5, T6, T7, T8,
  R1 extends string, R2 extends string, R3 extends string,
  R4 extends string, R5 extends string, R6 extends string,
  R7 extends string,
>(
  a1: TypedAction<T1, T2, R1>,
  a2: TypedAction<T2, T3, R2>,
  a3: TypedAction<T3, T4, R3>,
  a4: TypedAction<T4, T5, R4>,
  a5: TypedAction<T5, T6, R5>,
  a6: TypedAction<T6, T7, R6>,
  a7: TypedAction<T7, T8, R7>,
): TypedAction<T1, T8, R1 | R2 | R3 | R4 | R5 | R6 | R7>;
export function pipe<
  T1, T2, T3, T4, T5, T6, T7, T8, T9,
  R1 extends string, R2 extends string, R3 extends string,
  R4 extends string, R5 extends string, R6 extends string,
  R7 extends string, R8 extends string,
>(
  a1: TypedAction<T1, T2, R1>,
  a2: TypedAction<T2, T3, R2>,
  a3: TypedAction<T3, T4, R3>,
  a4: TypedAction<T4, T5, R4>,
  a5: TypedAction<T5, T6, R5>,
  a6: TypedAction<T6, T7, R6>,
  a7: TypedAction<T7, T8, R7>,
  a8: TypedAction<T8, T9, R8>,
): TypedAction<T1, T9, R1 | R2 | R3 | R4 | R5 | R6 | R7 | R8>;
export function pipe<
  T1, T2, T3, T4, T5, T6, T7, T8, T9, T10,
  R1 extends string, R2 extends string, R3 extends string,
  R4 extends string, R5 extends string, R6 extends string,
  R7 extends string, R8 extends string, R9 extends string,
>(
  a1: TypedAction<T1, T2, R1>,
  a2: TypedAction<T2, T3, R2>,
  a3: TypedAction<T3, T4, R3>,
  a4: TypedAction<T4, T5, R4>,
  a5: TypedAction<T5, T6, R5>,
  a6: TypedAction<T6, T7, R6>,
  a7: TypedAction<T7, T8, R7>,
  a8: TypedAction<T8, T9, R8>,
  a9: TypedAction<T9, T10, R9>,
): TypedAction<T1, T10, R1 | R2 | R3 | R4 | R5 | R6 | R7 | R8 | R9>;
export function pipe<
  T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11,
  R1 extends string, R2 extends string, R3 extends string,
  R4 extends string, R5 extends string, R6 extends string,
  R7 extends string, R8 extends string, R9 extends string,
  R10 extends string,
>(
  a1: TypedAction<T1, T2, R1>,
  a2: TypedAction<T2, T3, R2>,
  a3: TypedAction<T3, T4, R3>,
  a4: TypedAction<T4, T5, R4>,
  a5: TypedAction<T5, T6, R5>,
  a6: TypedAction<T6, T7, R6>,
  a7: TypedAction<T7, T8, R7>,
  a8: TypedAction<T8, T9, R8>,
  a9: TypedAction<T9, T10, R9>,
  a10: TypedAction<T10, T11, R10>,
): TypedAction<T1, T11, R1 | R2 | R3 | R4 | R5 | R6 | R7 | R8 | R9 | R10>;
export function pipe(...actions: Action[]): Action {
  if (actions.length === 0) return identity();
  if (actions.length === 1) return actions[0];
  return actions.reduceRight((rest, first) =>
    typedAction({ kind: "Chain", first, rest }) as Action,
  );
}
