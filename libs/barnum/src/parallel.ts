import type { Action, TypedAction } from "./ast.js";
import { typedAction } from "./ast.js";
import { constant } from "./builtins.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function parallel(): TypedAction<any, []>;
export function parallel<In, O1, R1 extends string>(
  a1: TypedAction<In, O1, R1>,
): TypedAction<In, [O1], R1>;
export function parallel<In, O1, O2, R1 extends string, R2 extends string>(
  a1: TypedAction<In, O1, R1>,
  a2: TypedAction<In, O2, R2>,
): TypedAction<In, [O1, O2], R1 | R2>;
export function parallel<
  In, O1, O2, O3,
  R1 extends string, R2 extends string, R3 extends string,
>(
  a1: TypedAction<In, O1, R1>,
  a2: TypedAction<In, O2, R2>,
  a3: TypedAction<In, O3, R3>,
): TypedAction<In, [O1, O2, O3], R1 | R2 | R3>;
export function parallel<
  In, O1, O2, O3, O4,
  R1 extends string, R2 extends string, R3 extends string, R4 extends string,
>(
  a1: TypedAction<In, O1, R1>,
  a2: TypedAction<In, O2, R2>,
  a3: TypedAction<In, O3, R3>,
  a4: TypedAction<In, O4, R4>,
): TypedAction<In, [O1, O2, O3, O4], R1 | R2 | R3 | R4>;
export function parallel<
  In, O1, O2, O3, O4, O5,
  R1 extends string, R2 extends string, R3 extends string,
  R4 extends string, R5 extends string,
>(
  a1: TypedAction<In, O1, R1>,
  a2: TypedAction<In, O2, R2>,
  a3: TypedAction<In, O3, R3>,
  a4: TypedAction<In, O4, R4>,
  a5: TypedAction<In, O5, R5>,
): TypedAction<In, [O1, O2, O3, O4, O5], R1 | R2 | R3 | R4 | R5>;
export function parallel<
  In, O1, O2, O3, O4, O5, O6,
  R1 extends string, R2 extends string, R3 extends string,
  R4 extends string, R5 extends string, R6 extends string,
>(
  a1: TypedAction<In, O1, R1>,
  a2: TypedAction<In, O2, R2>,
  a3: TypedAction<In, O3, R3>,
  a4: TypedAction<In, O4, R4>,
  a5: TypedAction<In, O5, R5>,
  a6: TypedAction<In, O6, R6>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6], R1 | R2 | R3 | R4 | R5 | R6>;
export function parallel<
  In, O1, O2, O3, O4, O5, O6, O7,
  R1 extends string, R2 extends string, R3 extends string,
  R4 extends string, R5 extends string, R6 extends string,
  R7 extends string,
>(
  a1: TypedAction<In, O1, R1>,
  a2: TypedAction<In, O2, R2>,
  a3: TypedAction<In, O3, R3>,
  a4: TypedAction<In, O4, R4>,
  a5: TypedAction<In, O5, R5>,
  a6: TypedAction<In, O6, R6>,
  a7: TypedAction<In, O7, R7>,
): TypedAction<
  In,
  [O1, O2, O3, O4, O5, O6, O7],
  R1 | R2 | R3 | R4 | R5 | R6 | R7
>;
export function parallel<
  In, O1, O2, O3, O4, O5, O6, O7, O8,
  R1 extends string, R2 extends string, R3 extends string,
  R4 extends string, R5 extends string, R6 extends string,
  R7 extends string, R8 extends string,
>(
  a1: TypedAction<In, O1, R1>,
  a2: TypedAction<In, O2, R2>,
  a3: TypedAction<In, O3, R3>,
  a4: TypedAction<In, O4, R4>,
  a5: TypedAction<In, O5, R5>,
  a6: TypedAction<In, O6, R6>,
  a7: TypedAction<In, O7, R7>,
  a8: TypedAction<In, O8, R8>,
): TypedAction<
  In,
  [O1, O2, O3, O4, O5, O6, O7, O8],
  R1 | R2 | R3 | R4 | R5 | R6 | R7 | R8
>;
export function parallel<
  In, O1, O2, O3, O4, O5, O6, O7, O8, O9,
  R1 extends string, R2 extends string, R3 extends string,
  R4 extends string, R5 extends string, R6 extends string,
  R7 extends string, R8 extends string, R9 extends string,
>(
  a1: TypedAction<In, O1, R1>,
  a2: TypedAction<In, O2, R2>,
  a3: TypedAction<In, O3, R3>,
  a4: TypedAction<In, O4, R4>,
  a5: TypedAction<In, O5, R5>,
  a6: TypedAction<In, O6, R6>,
  a7: TypedAction<In, O7, R7>,
  a8: TypedAction<In, O8, R8>,
  a9: TypedAction<In, O9, R9>,
): TypedAction<
  In,
  [O1, O2, O3, O4, O5, O6, O7, O8, O9],
  R1 | R2 | R3 | R4 | R5 | R6 | R7 | R8 | R9
>;
export function parallel<
  In, O1, O2, O3, O4, O5, O6, O7, O8, O9, O10,
  R1 extends string, R2 extends string, R3 extends string,
  R4 extends string, R5 extends string, R6 extends string,
  R7 extends string, R8 extends string, R9 extends string,
  R10 extends string,
>(
  a1: TypedAction<In, O1, R1>,
  a2: TypedAction<In, O2, R2>,
  a3: TypedAction<In, O3, R3>,
  a4: TypedAction<In, O4, R4>,
  a5: TypedAction<In, O5, R5>,
  a6: TypedAction<In, O6, R6>,
  a7: TypedAction<In, O7, R7>,
  a8: TypedAction<In, O8, R8>,
  a9: TypedAction<In, O9, R9>,
  a10: TypedAction<In, O10, R10>,
): TypedAction<
  In,
  [O1, O2, O3, O4, O5, O6, O7, O8, O9, O10],
  R1 | R2 | R3 | R4 | R5 | R6 | R7 | R8 | R9 | R10
>;
export function parallel(...actions: Action[]): Action {
  if (actions.length === 0) return constant([]);
  return typedAction({ kind: "Parallel", actions });
}
