import type { TypedAction } from "./ast.js";

export function pipe<T1, T2>(a1: TypedAction<T1, T2>): TypedAction<T1, T2>;
export function pipe<T1, T2, T3>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
): TypedAction<T1, T3>;
export function pipe<T1, T2, T3, T4>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
): TypedAction<T1, T4>;
export function pipe<T1, T2, T3, T4, T5>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
): TypedAction<T1, T5>;
export function pipe<T1, T2, T3, T4, T5, T6>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
): TypedAction<T1, T6>;
export function pipe<T1, T2, T3, T4, T5, T6, T7>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
): TypedAction<T1, T7>;
export function pipe<T1, T2, T3, T4, T5, T6, T7, T8>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
  a7: TypedAction<T7, T8>,
): TypedAction<T1, T8>;
export function pipe<T1, T2, T3, T4, T5, T6, T7, T8, T9>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
  a7: TypedAction<T7, T8>,
  a8: TypedAction<T8, T9>,
): TypedAction<T1, T9>;
export function pipe<T1, T2, T3, T4, T5, T6, T7, T8, T9, T10>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
  a7: TypedAction<T7, T8>,
  a8: TypedAction<T8, T9>,
  a9: TypedAction<T9, T10>,
): TypedAction<T1, T10>;
export function pipe<T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
  a7: TypedAction<T7, T8>,
  a8: TypedAction<T8, T9>,
  a9: TypedAction<T9, T10>,
  a10: TypedAction<T10, T11>,
): TypedAction<T1, T11>;
export function pipe(...actions: TypedAction[]): TypedAction {
  return { kind: "Pipe", actions };
}
