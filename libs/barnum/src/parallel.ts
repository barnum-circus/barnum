import type { TypedAction } from "./ast.js";

export function parallel<In, O1>(a1: TypedAction<In, O1>): TypedAction<In, [O1]>;
export function parallel<In, O1, O2>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
): TypedAction<In, [O1, O2]>;
export function parallel<In, O1, O2, O3>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
): TypedAction<In, [O1, O2, O3]>;
export function parallel<In, O1, O2, O3, O4>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
): TypedAction<In, [O1, O2, O3, O4]>;
export function parallel<In, O1, O2, O3, O4, O5>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
  a5: TypedAction<In, O5>,
): TypedAction<In, [O1, O2, O3, O4, O5]>;
export function parallel<In, O1, O2, O3, O4, O5, O6>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
  a5: TypedAction<In, O5>,
  a6: TypedAction<In, O6>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6]>;
export function parallel<In, O1, O2, O3, O4, O5, O6, O7>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
  a5: TypedAction<In, O5>,
  a6: TypedAction<In, O6>,
  a7: TypedAction<In, O7>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6, O7]>;
export function parallel<In, O1, O2, O3, O4, O5, O6, O7, O8>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
  a5: TypedAction<In, O5>,
  a6: TypedAction<In, O6>,
  a7: TypedAction<In, O7>,
  a8: TypedAction<In, O8>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6, O7, O8]>;
export function parallel<In, O1, O2, O3, O4, O5, O6, O7, O8, O9>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
  a5: TypedAction<In, O5>,
  a6: TypedAction<In, O6>,
  a7: TypedAction<In, O7>,
  a8: TypedAction<In, O8>,
  a9: TypedAction<In, O9>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6, O7, O8, O9]>;
export function parallel<In, O1, O2, O3, O4, O5, O6, O7, O8, O9, O10>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
  a5: TypedAction<In, O5>,
  a6: TypedAction<In, O6>,
  a7: TypedAction<In, O7>,
  a8: TypedAction<In, O8>,
  a9: TypedAction<In, O9>,
  a10: TypedAction<In, O10>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6, O7, O8, O9, O10]>;
export function parallel(...actions: TypedAction[]): TypedAction {
  return { kind: "Parallel", actions };
}
