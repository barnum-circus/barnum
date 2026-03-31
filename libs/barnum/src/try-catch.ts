import { type Action, type Pipeable, type TypedAction, typedAction } from "./ast.js";
import { allocateEffectId } from "./effect-id.js";

// ---------------------------------------------------------------------------
// tryCatch — type-level error handling via Handle/Perform
// ---------------------------------------------------------------------------

/**
 * HOAS combinator for type-level error handling. The body callback receives
 * a `throwError` token — a `TypedAction<TError, never>` that, when placed
 * in the pipeline, causes the Handle frame to discard the continuation and
 * run the recovery branch with the error payload.
 *
 * This handles **type-level errors only** — values returned by handlers via
 * the `Result` type. If a handler panics, throws a JavaScript exception, or
 * the runtime crashes, the existing error propagation path handles it.
 * tryCatch does not catch those. Analogous to Rust's `Result` vs `panic!`.
 *
 * Compiles to:
 *   Handle(effectId, handlerDag, body)
 *
 * Handler DAG:
 *   Chain(ExtractField("payload"), Chain(recovery, Tag("Discard")))
 *
 * The handler extracts the error payload, runs recovery, and tags the result
 * as Discard — the Handle frame tears down the body and exits with recovery's
 * result.
 */
export function tryCatch<TIn, TOut, TError>(
  body: (throwError: TypedAction<TError, never>) => Pipeable<TIn, TOut>,
  recovery: Pipeable<TError, TOut>,
): TypedAction<TIn, TOut> {
  const effectId = allocateEffectId();
  const throwError = typedAction<TError, never>({ kind: "Perform", effect_id: effectId });
  const bodyAction = body(throwError) as Action;

  const handlerDag: Action = {
    kind: "Chain",
    first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: "payload" } } },
    rest: {
      kind: "Chain",
      first: recovery as Action,
      rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Discard" } } },
    },
  };

  return typedAction({
    kind: "Handle",
    effect_id: effectId,
    handler: handlerDag,
    body: bodyAction,
  });
}

