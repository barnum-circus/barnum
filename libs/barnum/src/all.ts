import {
  type Action,
  type Pipeable,
  type TypedAction,
  typedAction,
} from "./ast.js";
import { constant } from "./builtins.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function all(): TypedAction<any, []>;
export function all<TInput, TOut1>(
  a1: Pipeable<TInput, TOut1>,
): TypedAction<TInput, [TOut1]>;
export function all<TInput, TOut1, TOut2>(
  a1: Pipeable<TInput, TOut1>,
  a2: Pipeable<TInput, TOut2>,
): TypedAction<TInput, [TOut1, TOut2]>;
export function all<TInput, TOut1, TOut2, TOut3>(
  a1: Pipeable<TInput, TOut1>,
  a2: Pipeable<TInput, TOut2>,
  a3: Pipeable<TInput, TOut3>,
): TypedAction<TInput, [TOut1, TOut2, TOut3]>;
export function all<TInput, TOut1, TOut2, TOut3, TOut4>(
  a1: Pipeable<TInput, TOut1>,
  a2: Pipeable<TInput, TOut2>,
  a3: Pipeable<TInput, TOut3>,
  a4: Pipeable<TInput, TOut4>,
): TypedAction<TInput, [TOut1, TOut2, TOut3, TOut4]>;
export function all<TInput, TOut1, TOut2, TOut3, TOut4, TOut5>(
  a1: Pipeable<TInput, TOut1>,
  a2: Pipeable<TInput, TOut2>,
  a3: Pipeable<TInput, TOut3>,
  a4: Pipeable<TInput, TOut4>,
  a5: Pipeable<TInput, TOut5>,
): TypedAction<TInput, [TOut1, TOut2, TOut3, TOut4, TOut5]>;
export function all<TInput, TOut1, TOut2, TOut3, TOut4, TOut5, TOut6>(
  a1: Pipeable<TInput, TOut1>,
  a2: Pipeable<TInput, TOut2>,
  a3: Pipeable<TInput, TOut3>,
  a4: Pipeable<TInput, TOut4>,
  a5: Pipeable<TInput, TOut5>,
  a6: Pipeable<TInput, TOut6>,
): TypedAction<TInput, [TOut1, TOut2, TOut3, TOut4, TOut5, TOut6]>;
export function all<TInput, TOut1, TOut2, TOut3, TOut4, TOut5, TOut6, TOut7>(
  a1: Pipeable<TInput, TOut1>,
  a2: Pipeable<TInput, TOut2>,
  a3: Pipeable<TInput, TOut3>,
  a4: Pipeable<TInput, TOut4>,
  a5: Pipeable<TInput, TOut5>,
  a6: Pipeable<TInput, TOut6>,
  a7: Pipeable<TInput, TOut7>,
): TypedAction<TInput, [TOut1, TOut2, TOut3, TOut4, TOut5, TOut6, TOut7]>;
export function all<
  TInput,
  TOut1,
  TOut2,
  TOut3,
  TOut4,
  TOut5,
  TOut6,
  TOut7,
  TOut8,
>(
  a1: Pipeable<TInput, TOut1>,
  a2: Pipeable<TInput, TOut2>,
  a3: Pipeable<TInput, TOut3>,
  a4: Pipeable<TInput, TOut4>,
  a5: Pipeable<TInput, TOut5>,
  a6: Pipeable<TInput, TOut6>,
  a7: Pipeable<TInput, TOut7>,
  a8: Pipeable<TInput, TOut8>,
): TypedAction<
  TInput,
  [TOut1, TOut2, TOut3, TOut4, TOut5, TOut6, TOut7, TOut8]
>;
export function all<
  TInput,
  TOut1,
  TOut2,
  TOut3,
  TOut4,
  TOut5,
  TOut6,
  TOut7,
  TOut8,
  TOut9,
>(
  a1: Pipeable<TInput, TOut1>,
  a2: Pipeable<TInput, TOut2>,
  a3: Pipeable<TInput, TOut3>,
  a4: Pipeable<TInput, TOut4>,
  a5: Pipeable<TInput, TOut5>,
  a6: Pipeable<TInput, TOut6>,
  a7: Pipeable<TInput, TOut7>,
  a8: Pipeable<TInput, TOut8>,
  a9: Pipeable<TInput, TOut9>,
): TypedAction<
  TInput,
  [TOut1, TOut2, TOut3, TOut4, TOut5, TOut6, TOut7, TOut8, TOut9]
>;
export function all<
  TInput,
  TOut1,
  TOut2,
  TOut3,
  TOut4,
  TOut5,
  TOut6,
  TOut7,
  TOut8,
  TOut9,
  TOut10,
>(
  a1: Pipeable<TInput, TOut1>,
  a2: Pipeable<TInput, TOut2>,
  a3: Pipeable<TInput, TOut3>,
  a4: Pipeable<TInput, TOut4>,
  a5: Pipeable<TInput, TOut5>,
  a6: Pipeable<TInput, TOut6>,
  a7: Pipeable<TInput, TOut7>,
  a8: Pipeable<TInput, TOut8>,
  a9: Pipeable<TInput, TOut9>,
  a10: Pipeable<TInput, TOut10>,
): TypedAction<
  TInput,
  [TOut1, TOut2, TOut3, TOut4, TOut5, TOut6, TOut7, TOut8, TOut9, TOut10]
>;
export function all(...actions: Action[]): Action {
  if (actions.length === 0) {
    return constant([]);
  }
  return typedAction({ kind: "All", actions });
}
