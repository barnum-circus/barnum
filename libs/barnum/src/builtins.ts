import type { TypedAction, LoopResult } from "./ast.js";
import {
  constant as constantHandler,
  drop as dropHandler,
  range as rangeHandler,
} from "./handlers/builtins.js";

/**
 * Typed combinators for structural data transformations.
 *
 * These will eventually serialize as Rust-native Builtin nodes.
 * For now they use placeholder Call nodes — the types are what matter.
 */

// Placeholder serialization until Builtin is added to the AST.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
function builtin(func: string): TypedAction<any, any> {
  return {
    kind: "Invoke",
    handler: { kind: "TypeScript", module: "__builtin__", func },
  };
}

// ---------------------------------------------------------------------------
// Identity — pass input through unchanged
// ---------------------------------------------------------------------------

export function identity<T>(): TypedAction<T, T> {
  return builtin("identity");
}

// ---------------------------------------------------------------------------
// Tag — wrap input as { kind, value }
// ---------------------------------------------------------------------------

export function tag<T, TKind extends string>(
  kind: TKind,
): TypedAction<T, { kind: TKind; value: T }> {
  return builtin(`tag:${kind}`);
}

// ---------------------------------------------------------------------------
// Loop signals
//
// These use `any` because their types depend on positional context (which
// pipe/loop they appear in), not on arguments. The loop's own signature
// validates the overall LoopResult<In, Out> shape.
// ---------------------------------------------------------------------------

// These use `any` because their types depend on positional context (which
// pipe/loop they appear in), not on arguments. Proper typing requires
// branch to narrow per-case inputs from the discriminated union — until
// then, the loop's own signature validates the overall LoopResult<In, Out>.
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function recur(): TypedAction<any, LoopResult<any, any>> {
  return builtin("tag:Continue");
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function done(): TypedAction<any, LoopResult<any, any>> {
  return builtin("tag:Break");
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
  return builtin("merge");
}

// ---------------------------------------------------------------------------
// Flatten — flatten a nested array one level
// ---------------------------------------------------------------------------

export function flatten<T>(): TypedAction<T[][], T[]> {
  return builtin("flatten");
}

// ---------------------------------------------------------------------------
// ExtractField — extract a single field from an object
// ---------------------------------------------------------------------------

export function extractField<
  TObj extends Record<string, unknown>,
  TField extends keyof TObj & string,
>(field: TField): TypedAction<TObj, TObj[TField]> {
  return builtin(`extractField:${field}`);
}

// ---------------------------------------------------------------------------
// Drop — discard pipeline value (enables transition to constant/range)
// ---------------------------------------------------------------------------

export function drop<T>(): TypedAction<T, never> {
  return dropHandler() as TypedAction<T, never>;
}

// ---------------------------------------------------------------------------
// DropResult — run an action for side effects, discard its output
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function dropResult<In>(action: TypedAction<In, any>): TypedAction<In, never> {
  return {
    kind: "Chain",
    first: action,
    rest: dropHandler(),
  } as TypedAction<In, never>;
}

// ---------------------------------------------------------------------------
// Constant — produce a fixed value (takes no pipeline input)
// ---------------------------------------------------------------------------

export function constant<T>(value: T): TypedAction<never, T> {
  return constantHandler({ stepConfig: { value } }) as TypedAction<never, T>;
}

// ---------------------------------------------------------------------------
// Range — produce an integer array [start, start+1, ..., end-1]
// ---------------------------------------------------------------------------

export function range(
  start: number,
  end: number,
): TypedAction<never, number[]> {
  return rangeHandler({
    stepConfig: { start, end },
  }) as TypedAction<never, number[]>;
}
