import type { TypedAction, LoopResult } from "./ast.js";
import { typedAction } from "./ast.js";
import { chain } from "./chain.js";

/**
 * Typed combinators for structural data transformations.
 *
 * All builtins emit `{ kind: "Builtin", builtin: { kind: ... } }` handler
 * kinds. The Rust scheduler executes them inline (no subprocess).
 */

// ---------------------------------------------------------------------------
// Constant — produce a fixed value (takes no pipeline input)
// ---------------------------------------------------------------------------

export function constant<T>(value: T): TypedAction<never, T> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Constant", value } },
  });
}

// ---------------------------------------------------------------------------
// Identity — pass input through unchanged
// ---------------------------------------------------------------------------

export function identity<T>(): TypedAction<T, T> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Identity" } },
  });
}

// ---------------------------------------------------------------------------
// Drop — discard pipeline value
// ---------------------------------------------------------------------------

export function drop<T>(): TypedAction<T, never> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Drop" } },
  });
}

// ---------------------------------------------------------------------------
// Tag — wrap input as { kind, value }
// ---------------------------------------------------------------------------

export function tag<T, TKind extends string>(
  kind: TKind,
): TypedAction<T, { kind: TKind; value: T }> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Tag", value: kind } },
  });
}

// ---------------------------------------------------------------------------
// Loop signals
//
// These use `any` because their types depend on positional context (which
// pipe/loop they appear in), not on arguments. The loop's own signature
// validates the overall LoopResult<In, Out> shape.
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function recur(): TypedAction<any, LoopResult<any, any>> {
  return tag("Continue") as TypedAction<any, LoopResult<any, any>>;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function done(): TypedAction<any, LoopResult<any, any>> {
  return tag("Break") as TypedAction<any, LoopResult<any, any>>;
}

// ---------------------------------------------------------------------------
// Merge — merge a tuple of objects into a single object
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type UnionToIntersection<U> = (U extends any ? (x: U) => void : never) extends (
  x: infer I,
) => void
  ? I
  : never;

export function merge<T extends Record<string, unknown>[]>(): TypedAction<
  T,
  UnionToIntersection<T[number]>
> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Merge" } },
  });
}

// ---------------------------------------------------------------------------
// Flatten — flatten a nested array one level
// ---------------------------------------------------------------------------

export function flatten<T>(): TypedAction<T[][], T[]> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Flatten" } },
  });
}

// ---------------------------------------------------------------------------
// ExtractField — extract a single field from an object
// ---------------------------------------------------------------------------

export function extractField<
  TObj extends Record<string, unknown>,
  TField extends keyof TObj & string,
>(field: TField): TypedAction<TObj, TObj[TField]> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: field } },
  });
}

// ---------------------------------------------------------------------------
// DropResult — run an action for side effects, discard its output
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function dropResult<In>(action: TypedAction<In, any>): TypedAction<In, never> {
  return chain(action, drop() as TypedAction<any, never>) as TypedAction<In, never>;
}

// ---------------------------------------------------------------------------
// Range — produce an integer array [start, start+1, ..., end-1]
// ---------------------------------------------------------------------------

export function range(start: number, end: number): TypedAction<never, number[]> {
  const result: number[] = [];
  for (let i = start; i < end; i++) result.push(i);
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Constant", value: result } },
  });
}
