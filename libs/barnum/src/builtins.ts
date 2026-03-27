import type { TypedAction, LoopResult } from "./core.js";

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
    kind: "Call",
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
// sequence/loop they appear in), not on arguments. The loop's own signature
// validates the overall LoopResult<In, Out> shape.
// ---------------------------------------------------------------------------

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
