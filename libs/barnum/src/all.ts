import {
  type Action,
  type Pipeable,
  type TypedAction,
  typedAction,
} from "./ast.js";
import { constant } from "./builtins.js";

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function all(): TypedAction<any, []>;
export function all<In, O1>(a1: Pipeable<In, O1>): TypedAction<In, [O1]>;
export function all<In, O1, O2>(
  a1: Pipeable<In, O1>,
  a2: Pipeable<In, O2>,
): TypedAction<In, [O1, O2]>;
export function all<In, O1, O2, O3>(
  a1: Pipeable<In, O1>,
  a2: Pipeable<In, O2>,
  a3: Pipeable<In, O3>,
): TypedAction<In, [O1, O2, O3]>;
export function all<In, O1, O2, O3, O4>(
  a1: Pipeable<In, O1>,
  a2: Pipeable<In, O2>,
  a3: Pipeable<In, O3>,
  a4: Pipeable<In, O4>,
): TypedAction<In, [O1, O2, O3, O4]>;
export function all<In, O1, O2, O3, O4, O5>(
  a1: Pipeable<In, O1>,
  a2: Pipeable<In, O2>,
  a3: Pipeable<In, O3>,
  a4: Pipeable<In, O4>,
  a5: Pipeable<In, O5>,
): TypedAction<In, [O1, O2, O3, O4, O5]>;
export function all<In, O1, O2, O3, O4, O5, O6>(
  a1: Pipeable<In, O1>,
  a2: Pipeable<In, O2>,
  a3: Pipeable<In, O3>,
  a4: Pipeable<In, O4>,
  a5: Pipeable<In, O5>,
  a6: Pipeable<In, O6>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6]>;
export function all<In, O1, O2, O3, O4, O5, O6, O7>(
  a1: Pipeable<In, O1>,
  a2: Pipeable<In, O2>,
  a3: Pipeable<In, O3>,
  a4: Pipeable<In, O4>,
  a5: Pipeable<In, O5>,
  a6: Pipeable<In, O6>,
  a7: Pipeable<In, O7>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6, O7]>;
export function all<In, O1, O2, O3, O4, O5, O6, O7, O8>(
  a1: Pipeable<In, O1>,
  a2: Pipeable<In, O2>,
  a3: Pipeable<In, O3>,
  a4: Pipeable<In, O4>,
  a5: Pipeable<In, O5>,
  a6: Pipeable<In, O6>,
  a7: Pipeable<In, O7>,
  a8: Pipeable<In, O8>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6, O7, O8]>;
export function all<In, O1, O2, O3, O4, O5, O6, O7, O8, O9>(
  a1: Pipeable<In, O1>,
  a2: Pipeable<In, O2>,
  a3: Pipeable<In, O3>,
  a4: Pipeable<In, O4>,
  a5: Pipeable<In, O5>,
  a6: Pipeable<In, O6>,
  a7: Pipeable<In, O7>,
  a8: Pipeable<In, O8>,
  a9: Pipeable<In, O9>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6, O7, O8, O9]>;
export function all<In, O1, O2, O3, O4, O5, O6, O7, O8, O9, O10>(
  a1: Pipeable<In, O1>,
  a2: Pipeable<In, O2>,
  a3: Pipeable<In, O3>,
  a4: Pipeable<In, O4>,
  a5: Pipeable<In, O5>,
  a6: Pipeable<In, O6>,
  a7: Pipeable<In, O7>,
  a8: Pipeable<In, O8>,
  a9: Pipeable<In, O9>,
  a10: Pipeable<In, O10>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6, O7, O8, O9, O10]>;
export function all(...actions: Action[]): Action {
  if (actions.length === 0) {
    return constant([]);
  }
  return typedAction({ kind: "All", actions });
}
