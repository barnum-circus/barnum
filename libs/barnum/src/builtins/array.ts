import {
  type Option as OptionT,
  type TypedAction,
  typedAction,
} from "../ast.js";

// ---------------------------------------------------------------------------
// GetIndex — extract a single element from an array by index
// ---------------------------------------------------------------------------

export function getIndex<TTuple extends unknown[], TIndex extends number>(
  index: TIndex,
): TypedAction<TTuple, OptionT<TTuple[TIndex]>> {
  return typedAction({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: { kind: "GetIndex", index },
    },
  });
}

// ---------------------------------------------------------------------------
// Flatten — flatten a nested array one level
// ---------------------------------------------------------------------------

export function flatten<TElement>(): TypedAction<TElement[][], TElement[]> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Flatten" } },
  });
}

// ---------------------------------------------------------------------------
// SplitFirst — head/tail decomposition of an array
// ---------------------------------------------------------------------------

/**
 * Deconstruct an array into its first element and the remaining elements.
 * `TElement[] → Option<[TElement, TElement[]]>`
 *
 * Returns `Some([first, rest])` for non-empty arrays, `None` for empty arrays.
 * This is the array equivalent of cons/uncons — enables recursive iteration
 * patterns via `loop` + `splitFirst` + `branch`.
 *
 * This is a builtin (SplitFirst) because it requires array-length branching
 * that can't be composed from existing AST nodes.
 */
export function splitFirst<TElement>(): TypedAction<
  TElement[],
  OptionT<[TElement, TElement[]]>
> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "SplitFirst" } },
  });
}

// ---------------------------------------------------------------------------
// SplitLast — init/last decomposition of an array
// ---------------------------------------------------------------------------

/**
 * Deconstruct an array into the leading elements and the last element.
 * `TElement[] → Option<[TElement[], TElement]>`
 *
 * Returns `Some([init, last])` for non-empty arrays, `None` for empty arrays.
 * Mirror of `splitFirst` — enables processing from the tail end.
 *
 * This is a builtin (SplitLast) because it requires array-length branching
 * that can't be composed from existing AST nodes.
 */
export function splitLast<TElement>(): TypedAction<
  TElement[],
  OptionT<[TElement[], TElement]>
> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "SplitLast" } },
  });
}

// ---------------------------------------------------------------------------
// Slice — extract a sub-array from start to end
// ---------------------------------------------------------------------------

/**
 * Slice an array from `start` (inclusive) to `end` (exclusive).
 * `T[] → T[]`
 *
 * Both indices are clamped to array length. If `end` is omitted, slices
 * to the end of the array. Returns empty array if `start >= end`.
 */
export function slice<TElement>(
  start: number,
  end?: number,
): TypedAction<TElement[], TElement[]> {
  const builtin: { kind: "Slice"; start: number; end?: number } =
    end === undefined
      ? { kind: "Slice", start }
      : { kind: "Slice", start, end };
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin },
  });
}

// ---------------------------------------------------------------------------
// Range — produce an integer array [start, start+1, ..., end-1]
// ---------------------------------------------------------------------------

export function range(start: number, end: number): TypedAction<any, number[]> {
  const result: number[] = [];
  for (let i = start; i < end; i++) {
    result.push(i);
  }
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Constant", value: result } },
  });
}
