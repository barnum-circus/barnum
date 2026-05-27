import {
  type BodyResult,
  type Pipeable,
  type TypedAction,
  type VarRef,
  bindInput,
  typedAction,
} from "../ast.js";

// ---------------------------------------------------------------------------
// WithResource — RAII-style create/action/dispose
// ---------------------------------------------------------------------------

/**
 * RAII-style resource management combinator.
 * ```
 * TIn → create → TResource
 *     → action(resourceRef, inputRef) → TOut
 *     → dispose(TResource) → ()
 *     → TOut
 * ```
 */
export function withResource<TIn, TResource, TOut>({
  create,
  action,
  dispose,
}: {
  create: Pipeable<TIn, TResource>;
  action: (resource: VarRef<TResource>, input: VarRef<TIn>) => BodyResult<TOut>;
  dispose: Pipeable<TResource, any>;
}): TypedAction<TIn, TOut> {
  return bindInput<TIn, TOut>((inputRef) =>
    typedAction<TIn, TResource>(create).bindInput<TOut>((resourceRef) =>
      typedAction<void, TOut>(action(resourceRef, inputRef)).bindInput<TOut>(
        (outputRef) =>
          typedAction<TResource, any>(dispose).drop().then(outputRef),
      ),
    ),
  );
}
