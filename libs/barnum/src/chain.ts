import { type Action, type Pipeable, type TypedAction, typedAction } from "./ast.js";

export function chain<T1, T2, T3, R1 extends string, R2 extends string>(
  first: Pipeable<T1, T2, R1>,
  rest: Pipeable<T2, T3, R2>,
): TypedAction<T1, T3, R1 | R2> {
  return typedAction({ kind: "Chain", first: first as Action, rest: rest as Action });
}
