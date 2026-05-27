import { type Pipeable, type TypedAction, bindInput } from "../ast.js";
import { all } from "../all.js";

// ---------------------------------------------------------------------------
// WithResource — RAII-style create/action/dispose
// ---------------------------------------------------------------------------

/**
 * RAII-style resource management combinator.
 * ```
 * TIn → create → TResource
 *     → action([TResource, TIn]) → TOut
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
  action: Pipeable<[TResource, TIn], TOut>;
  dispose: Pipeable<TResource, any>;
}): TypedAction<TIn, TOut> {
  return bindInput<TIn, TOut>((inputRef) =>
    inputRef.then(create).bindInput<TOut>((resourceRef) =>
      all(resourceRef, inputRef)
        .then(action)
        .bindInput<TOut>((outputRef) =>
          resourceRef.then(dispose).drop().then(outputRef),
        ),
    ),
  );
}
