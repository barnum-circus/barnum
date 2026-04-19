import { type Pipeable, type TypedAction, toAction } from "../ast.js";
import { chain } from "../chain.js";
import { all } from "../all.js";
import { identity } from "./scalar.js";
import { merge } from "./struct.js";
import { getIndex } from "./array.js";

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
  TDisposeOut = unknown,
>({
  create,
  action,
  dispose,
}: {
  create: Pipeable<TIn, TResource>;
  action: Pipeable<TResource & TIn, TOut>;
  dispose: Pipeable<TResource, TDisposeOut>;
}): TypedAction<TIn, TOut> {
  // Step 1: all(create, identity) → [TResource, TIn] → merge → TResource & TIn
  const acquireAndMerge = chain(
    toAction(all(create, identity())),
    toAction(merge()),
  );

  // Step 2: all(action, identity) → [TOut, TResource & TIn]
  const actionAndKeepMerged = all(toAction(action), toAction(identity()));

  // Step 3: all(getIndex(0).unwrap(), chain(getIndex(1).unwrap(), dispose)) → [TOut, unknown]
  const disposeAndKeepResult = all(
    toAction(getIndex(0).unwrap()),
    chain(toAction(getIndex(1).unwrap()), toAction(dispose)),
  );

  // Step 4: getIndex(0).unwrap() → TOut
  return chain(
    toAction(
      chain(
        toAction(
          chain(toAction(acquireAndMerge), toAction(actionAndKeepMerged)),
        ),
        toAction(disposeAndKeepResult),
      ),
    ),
    toAction(getIndex(0).unwrap()),
  ) as TypedAction<TIn, TOut>;
}
