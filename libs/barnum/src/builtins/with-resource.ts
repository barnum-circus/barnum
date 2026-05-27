import {
  type BodyResult,
  type Pipeable,
  type TypedAction,
  type VarRef,
  bindInput,
  toAction,
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
    (toAction(create) as TypedAction<TIn, TResource>).bindInput<TOut>(
      (resourceRef) =>
        (
          action(resourceRef, inputRef) as TypedAction<void, TOut>
        ).bindInput<TOut>((outputRef) =>
          (toAction(dispose) as TypedAction<TResource, any>)
            .drop()
            .then(outputRef),
        ),
    ),
  );
}
