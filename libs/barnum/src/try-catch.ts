import {
  type Action,
  type Pipeable,
  type TypedAction,
  typedAction,
  buildRestartBranchAction,
} from "./ast.js";
import { chain } from "./chain.js";
import { tag } from "./builtins.js";
import { allocateRestartHandlerId } from "./effect-id.js";

// ---------------------------------------------------------------------------
// tryCatch — type-level error handling via restart+Branch
// ---------------------------------------------------------------------------

/**
 * HOAS combinator for type-level error handling. The body callback receives
 * a `throwError` token — a `TypedAction<TError, never>` that, when placed
 * in the pipeline, tags the error as Break, performs to the handler, which
 * restarts the body. The body-level Branch routes to the recovery arm.
 *
 * This handles **type-level errors only** — values returned by handlers via
 * the `Result` type. If a handler panics, throws a JavaScript exception, or
 * the runtime crashes, the existing error propagation path handles it.
 * tryCatch does not catch those. Analogous to Rust's `Result` vs `panic!`.
 *
 * Compiled form (restart+Branch, same substrate as loop/earlyReturn):
 *   `Chain(Tag("Continue"),`
 *     `RestartHandle(id, GetIndex(0),`
 *       `Branch({ Continue: body, Break: recovery })))`
 *
 * throwError = `Chain(Tag("Break"), RestartPerform(id))`
 *
 * When throwError fires: error tagged Break → `RestartPerform` → handler extracts
 * payload → body restarts → Branch takes Break arm → recovery receives error.
 */
export function tryCatch<TIn, TOut, TError>(
  body: (throwError: TypedAction<TError, never>) => Pipeable<TIn, TOut>,
  recovery: Pipeable<TError, TOut>,
): TypedAction<TIn, TOut> {
  const restartHandlerId = allocateRestartHandlerId();

  const throwError = typedAction<TError, never>(
    chain(
      tag("Break") as any,
      {
        kind: "RestartPerform",
        restart_handler_id: restartHandlerId,
      } as any,
    ) as Action,
  );

  const bodyAction = body(throwError) as Action;

  return typedAction(
    buildRestartBranchAction(restartHandlerId, bodyAction, recovery as Action),
  );
}
