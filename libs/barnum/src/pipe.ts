import {
  type Action,
  type PipeIn,
  type Pipeable,
  type TypedAction,
  toAction,
} from "./ast.js";
import { chain } from "./chain.js";
import { identity } from "./builtins/index.js";

export function pipe<TStep1, TStep2>(
  a1: Pipeable<TStep1, TStep2>,
): TypedAction<PipeIn<TStep1>, TStep2>;
export function pipe<TStep1, TStep2, TStep3>(
  a1: Pipeable<TStep1, TStep2>,
  a2: Pipeable<TStep2, TStep3>,
): TypedAction<PipeIn<TStep1>, TStep3>;
export function pipe<TStep1, TStep2, TStep3, TStep4>(
  a1: Pipeable<TStep1, TStep2>,
  a2: Pipeable<TStep2, TStep3>,
  a3: Pipeable<TStep3, TStep4>,
): TypedAction<PipeIn<TStep1>, TStep4>;
export function pipe<TStep1, TStep2, TStep3, TStep4, TStep5>(
  a1: Pipeable<TStep1, TStep2>,
  a2: Pipeable<TStep2, TStep3>,
  a3: Pipeable<TStep3, TStep4>,
  a4: Pipeable<TStep4, TStep5>,
): TypedAction<PipeIn<TStep1>, TStep5>;
export function pipe<TStep1, TStep2, TStep3, TStep4, TStep5, TStep6>(
  a1: Pipeable<TStep1, TStep2>,
  a2: Pipeable<TStep2, TStep3>,
  a3: Pipeable<TStep3, TStep4>,
  a4: Pipeable<TStep4, TStep5>,
  a5: Pipeable<TStep5, TStep6>,
): TypedAction<PipeIn<TStep1>, TStep6>;
export function pipe<TStep1, TStep2, TStep3, TStep4, TStep5, TStep6, TStep7>(
  a1: Pipeable<TStep1, TStep2>,
  a2: Pipeable<TStep2, TStep3>,
  a3: Pipeable<TStep3, TStep4>,
  a4: Pipeable<TStep4, TStep5>,
  a5: Pipeable<TStep5, TStep6>,
  a6: Pipeable<TStep6, TStep7>,
): TypedAction<PipeIn<TStep1>, TStep7>;
export function pipe<
  TStep1,
  TStep2,
  TStep3,
  TStep4,
  TStep5,
  TStep6,
  TStep7,
  TStep8,
>(
  a1: Pipeable<TStep1, TStep2>,
  a2: Pipeable<TStep2, TStep3>,
  a3: Pipeable<TStep3, TStep4>,
  a4: Pipeable<TStep4, TStep5>,
  a5: Pipeable<TStep5, TStep6>,
  a6: Pipeable<TStep6, TStep7>,
  a7: Pipeable<TStep7, TStep8>,
): TypedAction<PipeIn<TStep1>, TStep8>;
export function pipe<
  TStep1,
  TStep2,
  TStep3,
  TStep4,
  TStep5,
  TStep6,
  TStep7,
  TStep8,
  TStep9,
>(
  a1: Pipeable<TStep1, TStep2>,
  a2: Pipeable<TStep2, TStep3>,
  a3: Pipeable<TStep3, TStep4>,
  a4: Pipeable<TStep4, TStep5>,
  a5: Pipeable<TStep5, TStep6>,
  a6: Pipeable<TStep6, TStep7>,
  a7: Pipeable<TStep7, TStep8>,
  a8: Pipeable<TStep8, TStep9>,
): TypedAction<PipeIn<TStep1>, TStep9>;
export function pipe<
  TStep1,
  TStep2,
  TStep3,
  TStep4,
  TStep5,
  TStep6,
  TStep7,
  TStep8,
  TStep9,
  TStep10,
>(
  a1: Pipeable<TStep1, TStep2>,
  a2: Pipeable<TStep2, TStep3>,
  a3: Pipeable<TStep3, TStep4>,
  a4: Pipeable<TStep4, TStep5>,
  a5: Pipeable<TStep5, TStep6>,
  a6: Pipeable<TStep6, TStep7>,
  a7: Pipeable<TStep7, TStep8>,
  a8: Pipeable<TStep8, TStep9>,
  a9: Pipeable<TStep9, TStep10>,
): TypedAction<PipeIn<TStep1>, TStep10>;
export function pipe<
  TStep1,
  TStep2,
  TStep3,
  TStep4,
  TStep5,
  TStep6,
  TStep7,
  TStep8,
  TStep9,
  TStep10,
  TStep11,
>(
  a1: Pipeable<TStep1, TStep2>,
  a2: Pipeable<TStep2, TStep3>,
  a3: Pipeable<TStep3, TStep4>,
  a4: Pipeable<TStep4, TStep5>,
  a5: Pipeable<TStep5, TStep6>,
  a6: Pipeable<TStep6, TStep7>,
  a7: Pipeable<TStep7, TStep8>,
  a8: Pipeable<TStep8, TStep9>,
  a9: Pipeable<TStep9, TStep10>,
  a10: Pipeable<TStep10, TStep11>,
): TypedAction<PipeIn<TStep1>, TStep11>;
export function pipe(...actions: Action[]): Action {
  if (actions.length === 0) {
    return identity();
  }
  if (actions.length === 1) {
    return actions[0];
  }
  return actions.reduceRight((rest, first) =>
    toAction(chain(toAction(first), toAction(rest))),
  );
}
