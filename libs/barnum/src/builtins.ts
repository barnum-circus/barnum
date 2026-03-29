import { type Action, type TypedAction, typedAction } from "./ast.js";
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

export function constant<TValue>(value: TValue): TypedAction<never, TValue> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Constant", value } },
  });
}

// ---------------------------------------------------------------------------
// Identity — pass input through unchanged
// ---------------------------------------------------------------------------

export function identity<TValue>(): TypedAction<TValue, TValue> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Identity" } },
  });
}

// ---------------------------------------------------------------------------
// Drop — discard pipeline value
// ---------------------------------------------------------------------------

export function drop<TValue>(): TypedAction<TValue, never> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Drop" } },
  });
}

// ---------------------------------------------------------------------------
// Tag — wrap input as { kind, value }
// ---------------------------------------------------------------------------

export function tag<TValue, TKind extends string>(
  kind: TKind,
): TypedAction<TValue, { kind: TKind; value: TValue }> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Tag", value: kind } },
  });
}

// ---------------------------------------------------------------------------
// Loop signals
//
// Return individual union members ({ kind: "Continue" } or { kind: "Break" })
// rather than the full LoopResult<TContinue, TBreak> with `never` in the
// opposite slot. This lets branch unify Out as the union of both members.
// ---------------------------------------------------------------------------

export function recur<TValue>(): TypedAction<
  TValue,
  { kind: "Continue"; value: TValue }
> {
  return tag("Continue");
}

export function done<TValue>(): TypedAction<
  TValue,
  { kind: "Break"; value: TValue }
> {
  return tag("Break");
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

export function merge<
  TObjects extends Record<string, unknown>[],
>(): TypedAction<TObjects, UnionToIntersection<TObjects[number]>> {
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Merge" } },
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
// ExtractField — extract a single field from an object
// ---------------------------------------------------------------------------

export function extractField<
  TObj extends Record<string, unknown>,
  TField extends keyof TObj & string,
>(field: TField): TypedAction<TObj, TObj[TField]> {
  return typedAction({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: { kind: "ExtractField", value: field },
    },
  });
}

// ---------------------------------------------------------------------------
// ExtractIndex — extract a single element from an array by index
// ---------------------------------------------------------------------------

export function extractIndex<
  TTuple extends unknown[],
  TIndex extends number,
>(index: TIndex): TypedAction<TTuple, TTuple[TIndex]> {
  return typedAction({
    kind: "Invoke",
    handler: {
      kind: "Builtin",
      builtin: { kind: "ExtractIndex", value: index },
    },
  });
}

// ---------------------------------------------------------------------------
// DropResult — run an action for side effects, discard its output
// ---------------------------------------------------------------------------

export function dropResult<TInput, TOutput>(
  action: TypedAction<TInput, TOutput>,
): TypedAction<TInput, never> {
  return chain(action, drop());
}

// ---------------------------------------------------------------------------
// WithResource — RAII-style create/action/dispose
// ---------------------------------------------------------------------------

/**
 * RAII-style resource management combinator.
 *
 * Runs `create` to acquire a resource, then merges the resource with the
 * original input into a flat object (`TResource & TIn`) for the action.
 * After the action completes, `dispose` receives the resource for cleanup.
 * The overall combinator returns the action's output.
 *
 * ```
 * TIn → create → TResource
 *     → merge(TResource, TIn) → TResource & TIn
 *     → action(TResource & TIn) → TOut
 *     → dispose(TResource) → (discarded)
 *     → TOut
 * ```
 */
export function withResource<
  TIn extends Record<string, unknown>,
  TResource extends Record<string, unknown>,
  TOut,
>({
  create,
  action,
  dispose,
}: {
  create: TypedAction<TIn, TResource>;
  action: TypedAction<TResource & TIn, TOut>;
  dispose: TypedAction<TResource, unknown>;
}): TypedAction<TIn, TOut> {
  const mergeBuiltin: Action = {
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Merge" } },
  };

  // Step 1: parallel(create, identity) → [TResource, TIn] → merge → TResource & TIn
  const acquireAndMerge = chain(
    typedAction<TIn, [TResource, TIn]>({
      kind: "Parallel",
      actions: [create as Action, identity() as Action],
    }),
    typedAction<[TResource, TIn], TResource & TIn>(mergeBuiltin),
  );

  // Step 2: parallel(action, identity) → [TOut, TResource & TIn]
  // Keep merged object so dispose can access resource fields.
  const actionAndKeepMerged = typedAction<TResource & TIn, [TOut, TResource & TIn]>({
    kind: "Parallel",
    actions: [action as Action, identity() as Action],
  });

  // Step 3: parallel(extractIndex(0), chain(extractIndex(1), dispose)) → [TOut, unknown]
  const disposeAndKeepResult = typedAction<[TOut, TResource & TIn], [TOut, unknown]>({
    kind: "Parallel",
    actions: [
      extractIndex<[TOut, TResource & TIn], 0>(0) as Action,
      chain(
        extractIndex<[TOut, TResource & TIn], 1>(1),
        dispose as TypedAction<TResource & TIn, unknown>,
      ) as Action,
    ],
  });

  // Step 4: extractIndex(0) → TOut
  return chain(
    chain(chain(acquireAndMerge, actionAndKeepMerged), disposeAndKeepResult),
    extractIndex<[TOut, unknown], 0>(0),
  ) as TypedAction<TIn, TOut>;
}

// ---------------------------------------------------------------------------
// Augment — run a transform, merge its output back into the original input
// ---------------------------------------------------------------------------

/**
 * Run `action` on the input, then merge the action's output fields back
 * into the original input object. The action must accept exactly `TInput`.
 * Use `pick` inside the action's pipe if the inner handler needs a subset.
 *
 * Example:
 *   augment(pipe(pick("file"), migrate))
 *   // { file, outputPath } → { file, outputPath, content, migrated }
 */
export function augment<
  TInput extends Record<string, unknown>,
  TOutput extends Record<string, unknown>,
  TRefs extends string = never,
>(
  action: TypedAction<TInput, TOutput, TRefs>,
): TypedAction<TInput, TInput & TOutput, TRefs> {
  // Construct parallel(action, identity()) inline to avoid circular import
  // with parallel.ts (which imports constant from this file).
  const parallelNode = typedAction<TInput, [TOutput, TInput], TRefs>({
    kind: "Parallel",
    actions: [action as Action, identity() as Action],
  });
  // UnionToIntersection<A | B> is semantically A & B, but TypeScript
  // can't reduce this at the generic level. Safe cast.
  return chain(parallelNode, merge()) as TypedAction<TInput, TInput & TOutput, TRefs>;
}

// ---------------------------------------------------------------------------
// Tap — run an action for side effects, preserve original input
// ---------------------------------------------------------------------------

/**
 * Run `action` on the input for its side effects, then discard the action's
 * output and return the original input unchanged. The action must accept
 * exactly `TInput`. Use `pick` inside the action's pipe if the inner
 * handler needs a subset.
 *
 * Constraint: input must be an object (uses augment internally, which
 * relies on parallel + merge).
 *
 * Example:
 *   pipe(tap(pipe(pick("worktreePath", "description"), implement)), createPR)
 */
export function tap<TInput extends Record<string, unknown>>(
  action: TypedAction<TInput, unknown>,
): TypedAction<TInput, TInput> {
  const voided = chain(action, constant({}) as TypedAction<unknown, Record<string, unknown>>);
  return augment(voided) as TypedAction<TInput, TInput>;
}

// ---------------------------------------------------------------------------
// Range — produce an integer array [start, start+1, ..., end-1]
// ---------------------------------------------------------------------------

export function range(
  start: number,
  end: number,
): TypedAction<never, number[]> {
  const result: number[] = [];
  for (let i = start; i < end; i++) {
    result.push(i);
  }
  return typedAction({
    kind: "Invoke",
    handler: { kind: "Builtin", builtin: { kind: "Constant", value: result } },
  });
}
