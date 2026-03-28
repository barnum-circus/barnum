import type { Action, TypedAction } from "./ast.js";

export function chain<T1, T2, T3, R1 extends string, R2 extends string>(
  first: TypedAction<T1, T2, R1>,
  rest: TypedAction<T2, T3, R2>,
): TypedAction<T1, T3, R1 | R2> {
  return { kind: "Chain", first, rest } as TypedAction<T1, T3, R1 | R2>;
}
