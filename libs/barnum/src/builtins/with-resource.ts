import { type Pipeable, type TypedAction, toAction } from "../ast.js";
import { chain } from "../chain.js";
import { all } from "../all.js";
import { identity } from "./scalar.js";
import { getIndex } from "./array.js";

// ---------------------------------------------------------------------------
// WithResource — RAII-style create/action/dispose
// ---------------------------------------------------------------------------

/**
 * RAII-style resource management combinator.
 *
 * Runs `create` to acquire a resource, then passes `[TResource, TIn]`
 * as a tuple to the action. After the action completes, `dispose`
 * receives the resource for cleanup. The overall combinator returns
 * the action's output.
 *
 * ```
 * TIn → create → TResource
 *     → action([TResource, TIn]) → TOut
 *     → dispose(TResource) → (discarded)
 *     → TOut
 * ```
 */
export function withResource<TIn, TResource, TOut, TDisposeOut = unknown>({
  create,
  action,
  dispose,
}: {
  create: Pipeable<TIn, TResource>;
  action: Pipeable<[TResource, TIn], TOut>;
  dispose: Pipeable<TResource, TDisposeOut>;
}): TypedAction<TIn, TOut> {
  // Step 1: all(create, identity) → [TResource, TIn]
  const acquireParallel = all(create, identity());

  // Step 2: all(action, getIndex(0)) → [TOut, TResource]
  // Run action on the tuple, keep raw TResource for dispose
  const runActionKeepResource = all(
    toAction(action),
    toAction(getIndex(0).unwrap()),
  );

  // Step 3: all(getIndex(0) → TOut, getIndex(1) → dispose) → [TOut, TDisposeOut]
  const disposeAndKeepResult = all(
    toAction(getIndex(0).unwrap()),
    chain(toAction(getIndex(1).unwrap()), toAction(dispose)),
  );

  // Step 4: getIndex(0).unwrap() → TOut
  return chain(
    toAction(
      chain(
        toAction(
          chain(toAction(acquireParallel), toAction(runActionKeepResource)),
        ),
        toAction(disposeAndKeepResult),
      ),
    ),
    toAction(getIndex(0).unwrap()),
  ) as TypedAction<TIn, TOut>;
}
