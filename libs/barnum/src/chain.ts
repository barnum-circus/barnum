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
  const result = typedAction<T1, T3>({
    kind: "Chain",
    first: first as Action,
    rest: rest as Action,
  });
  // Propagate __union from the output-determining action so that
  // x.then(Option.map(f)).isSome() works without explicit withUnion.
  const restUnion = (rest as TypedAction).__union;
  if (restUnion) {
    result.__union = restUnion;
  }
  return result;
}
