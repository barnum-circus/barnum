import {
  type Action,
  type PipeIn,
  type Pipeable,
  type TypedAction,
  typedAction,
} from "./ast.js";
import { identity } from "./builtins.js";

export function pipe<T1, T2>(a1: Pipeable<T1, T2>): TypedAction<PipeIn<T1>, T2>;
export function pipe<T1, T2, T3>(
  a1: Pipeable<T1, T2>,
  a2: Pipeable<T2, T3>,
): TypedAction<PipeIn<T1>, T3>;
export function pipe<T1, T2, T3, T4>(
  a1: Pipeable<T1, T2>,
  a2: Pipeable<T2, T3>,
  a3: Pipeable<T3, T4>,
): TypedAction<PipeIn<T1>, T4>;
export function pipe<T1, T2, T3, T4, T5>(
  a1: Pipeable<T1, T2>,
  a2: Pipeable<T2, T3>,
  a3: Pipeable<T3, T4>,
  a4: Pipeable<T4, T5>,
): TypedAction<PipeIn<T1>, T5>;
export function pipe<T1, T2, T3, T4, T5, T6>(
  a1: Pipeable<T1, T2>,
  a2: Pipeable<T2, T3>,
  a3: Pipeable<T3, T4>,
  a4: Pipeable<T4, T5>,
  a5: Pipeable<T5, T6>,
): TypedAction<PipeIn<T1>, T6>;
export function pipe<T1, T2, T3, T4, T5, T6, T7>(
  a1: Pipeable<T1, T2>,
  a2: Pipeable<T2, T3>,
  a3: Pipeable<T3, T4>,
  a4: Pipeable<T4, T5>,
  a5: Pipeable<T5, T6>,
  a6: Pipeable<T6, T7>,
): TypedAction<PipeIn<T1>, T7>;
export function pipe<T1, T2, T3, T4, T5, T6, T7, T8>(
  a1: Pipeable<T1, T2>,
  a2: Pipeable<T2, T3>,
  a3: Pipeable<T3, T4>,
  a4: Pipeable<T4, T5>,
  a5: Pipeable<T5, T6>,
  a6: Pipeable<T6, T7>,
  a7: Pipeable<T7, T8>,
): TypedAction<PipeIn<T1>, T8>;
export function pipe<T1, T2, T3, T4, T5, T6, T7, T8, T9>(
  a1: Pipeable<T1, T2>,
  a2: Pipeable<T2, T3>,
  a3: Pipeable<T3, T4>,
  a4: Pipeable<T4, T5>,
  a5: Pipeable<T5, T6>,
  a6: Pipeable<T6, T7>,
  a7: Pipeable<T7, T8>,
  a8: Pipeable<T8, T9>,
): TypedAction<PipeIn<T1>, T9>;
export function pipe<T1, T2, T3, T4, T5, T6, T7, T8, T9, T10>(
  a1: Pipeable<T1, T2>,
  a2: Pipeable<T2, T3>,
  a3: Pipeable<T3, T4>,
  a4: Pipeable<T4, T5>,
  a5: Pipeable<T5, T6>,
  a6: Pipeable<T6, T7>,
  a7: Pipeable<T7, T8>,
  a8: Pipeable<T8, T9>,
  a9: Pipeable<T9, T10>,
): TypedAction<PipeIn<T1>, T10>;
export function pipe<T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11>(
  a1: Pipeable<T1, T2>,
  a2: Pipeable<T2, T3>,
  a3: Pipeable<T3, T4>,
  a4: Pipeable<T4, T5>,
  a5: Pipeable<T5, T6>,
  a6: Pipeable<T6, T7>,
  a7: Pipeable<T7, T8>,
  a8: Pipeable<T8, T9>,
  a9: Pipeable<T9, T10>,
  a10: Pipeable<T10, T11>,
): TypedAction<PipeIn<T1>, T11>;
export function pipe(...actions: Action[]): Action {
  if (actions.length === 0) {
    return identity();
  }
  if (actions.length === 1) {
    return actions[0];
  }
  return actions.reduceRight(
    (rest, first) => typedAction({ kind: "Chain", first, rest }) as Action,
  );
}
