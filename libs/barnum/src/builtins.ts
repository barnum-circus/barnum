import { type Action, type ChainableAction, type TypedAction, typedAction } from "./ast.js";
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

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function dropResult<TInput>(
  action: TypedAction<TInput, any>,
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
 * After the action completes, `dispose` receives the merged object for
 * cleanup (it can access resource fields it needs). The overall combinator
 * returns the action's output.
 *
 * ```
 * TIn → create → TResource
 *     → merge(TResource, TIn) → TResource & TIn
 *     → action(TResource & TIn) → TOut
 *     → dispose(TResource & TIn) → (discarded)
 *     → TOut
 * ```
 *
 * The action receives a flat merged object so handlers can access both
 * resource fields (e.g. worktreePath, branch) and input fields (e.g.
 * file, description) without manual merge().
 *
 * TIn is inferred from create's input type (which may be narrower than
 * the full pipeline data type). The return type uses `__in?: any` to
 * bypass __in invariance — pipe's contravariant __phantom_in check is
 * sufficient to verify the pipeline data is a supertype of create's input.
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
  create: ChainableAction<TIn, TResource>;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  action: ChainableAction<any, TOut>;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  dispose: ChainableAction<NoInfer<TResource>, any>;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
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
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    typedAction<any, TResource & TIn>(mergeBuiltin),
  );

  // Step 2: parallel(action, identity) → [TOut, TResource & TIn]
  // Keep merged object so dispose can access resource fields.
  const actionAndKeepMerged = typedAction<TResource & TIn, [TOut, TResource & TIn]>({
    kind: "Parallel",
    actions: [action as Action, identity() as Action],
  });

  // Step 3: parallel(extractIndex(0), chain(extractIndex(1), dispose)) → [TOut, unknown]
  // Dispose receives the full merged object; TResource & TIn extends TResource,
  // so the dispose handler can access all resource fields it needs.
  const disposeAndKeepResult = typedAction<[TOut, TResource & TIn], [TOut, unknown]>({
    kind: "Parallel",
    actions: [
      extractIndex(0) as Action,
      chain(
        // eslint-disable-next-line @typescript-eslint/no-explicit-any
        extractIndex(1) as TypedAction<any, TResource & TIn>,
        dispose as Action as TypedAction<TResource & TIn, unknown>,
      ) as Action,
    ],
  });

  // Step 4: extractIndex(0) → TOut
  //
  // Cast to `{ __in?: any }` to bypass __in invariance. TIn is inferred
  // from create's input (e.g. {description: string}), which is narrower
  // than the actual pipeline data (e.g. Refactor). Pipe's contravariant
  // __phantom_in check correctly verifies compatibility; __in's covariant
  // check would incorrectly reject the wider pipeline type.
  return chain(
    chain(chain(acquireAndMerge, actionAndKeepMerged), disposeAndKeepResult),
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
    extractIndex(0) as TypedAction<any, TOut>,
    // eslint-disable-next-line @typescript-eslint/no-explicit-any
  ) as TypedAction<TIn, TOut> & { __in?: any };
}

// ---------------------------------------------------------------------------
// Augment — run a transform, merge its output back into the original input
// ---------------------------------------------------------------------------

/**
 * Run `action` on the input, then merge the action's output fields back
 * into the original input object. Replaces the verbose
 * `parallel(action, identity()) → merge()` pattern.
 *
 * `TInput` is inferred from the pipeline context (not from the action's
 * input type), so augment preserves the full pipeline type. The action's
 * input is unchecked at compile time — runtime zod validators catch
 * mismatches.
 *
 * Example:
 *   augment(pipe(extractField("file"), migrate({ to: "Typescript" })))
 *   // { file, outputPath } → { content, file, outputPath }
 */
export function augment<
  TInput extends Record<string, unknown>,
  TOutput extends Record<string, unknown>,
  TRefs extends string = never,
>(
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  action: TypedAction<any, TOutput, TRefs>,
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
 * output and return the original input unchanged. Useful for side-effectful
 * steps (type-checking, committing) in a pipeline that needs to preserve
 * context.
 *
 * `TInput` is inferred from the pipeline context (not from the action's
 * input type), so tap preserves the full pipeline type through side-effectful
 * steps. The action's input is unchecked at compile time — runtime zod
 * validators catch mismatches.
 *
 * Constraint: input must be an object (uses augment internally, which
 * relies on parallel + merge).
 *
 * Example:
 *   pipe(tap(implement), tap(commit), createPR)
 *   // context flows through implement and commit unchanged
 */
export function tap<TInput extends Record<string, unknown>>(
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  action: TypedAction<any, any, any>,
): TypedAction<TInput, TInput> {
  // Replace action's output with {} via constant({}), then augment.
  // augment runs parallel(voided, identity()) → merge().
  // merge([{}, input]) = input, so the original value passes through.
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const voided = chain(action, constant({}) as TypedAction<any, Record<string, unknown>>);
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
