import {
  type Action,
  type Pipeable,
  type TypedAction,
  typedAction,
} from "./ast.js";

export function chain<T1, T2, T3>(
  first: Pipeable<T1, T2>,
  rest: Pipeable<T2, T3>,
): TypedAction<T1, T3> {
  return typedAction({
    kind: "Chain",
    first: first as Action,
    rest: rest as Action,
  });
}
