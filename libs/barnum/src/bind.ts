import { type Action, type ExtractInput, type ExtractOutput, type TypedAction, typedAction } from "./ast.js";
import { identity, drop } from "./builtins.js";
import { allocateEffectId } from "./effect-id.js";
import { pipe } from "./pipe.js";

// ---------------------------------------------------------------------------
// VarRef — typed reference to a bound value
// ---------------------------------------------------------------------------

/**
 * A typed reference to a bound value. Output is `TValue`.
 *
 * Use `.then()` (not `pipe()`) when chaining a VarRef into a generic
 * action like `pick` or `extractField` — pipe overloads can't infer
 * the generic's type parameter from the VarRef's output.
 */
export type VarRef<TValue> = TypedAction<never, TValue>;

function createVarRef<TValue>(effectId: number): VarRef<TValue> {
  return typedAction({ kind: "Perform", effect_id: effectId });
}

// ---------------------------------------------------------------------------
// InferVarRefs — map bindings to VarRef types
// ---------------------------------------------------------------------------

/**
 * Maps each binding's output type to a VarRef. TypeScript resolves
 * ExtractOutput from each binding expression.
 *
 * Constraint is `Action[]` (not `Pipeable<any, any>[]`) because
 * `TypedAction<never, X>` (e.g. from `constant()`) fails the invariant
 * `__phantom_in` check against `Pipeable<any, any>` on the 9-variant
 * Action union. Using raw `Action[]` avoids the phantom field
 * assignability issue while `ExtractOutput` still extracts the correct
 * output type from the phantom fields on the concrete types.
 */
export type InferVarRefs<TBindings extends Action[]> = {
  [K in keyof TBindings]: VarRef<ExtractOutput<TBindings[K]>>;
};

// ---------------------------------------------------------------------------
// readVar — handler DAG for the nth binding
// ---------------------------------------------------------------------------

/**
 * Returns an action that extracts the nth value from the Handle's state
 * tuple and resumes with it. When a Perform fires, the engine calls the
 * handler with `{ payload, state }`. For bind, `state` is the full All
 * output tuple. The handler extracts `state[n]` and wraps it as
 * `{ kind: "Resume", value: state[n] }`.
 *
 * Expanded AST: Chain(ExtractField("state"), Chain(ExtractIndex(n), Tag("Resume")))
 */
function readVar(n: number): Action {
  return {
    kind: "Chain",
    first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractField", value: "state" } } },
    rest: {
      kind: "Chain",
      first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractIndex", value: n } } },
      rest: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Tag", value: "Resume" } } },
    },
  };
}

// ---------------------------------------------------------------------------
// bind — the user-facing function
// ---------------------------------------------------------------------------

/**
 * Bind concurrent values as VarRefs available throughout the body.
 *
 * All bindings are actions (Pipeable) evaluated concurrently with the
 * pipeline input. The body callback receives an array of VarRefs,
 * one per binding.
 *
 * Compiles to:
 *   Chain(
 *     All(...bindings, Identity),
 *     Handle(e0, readVar(0),
 *       Handle(e1, readVar(1),
 *         Chain(ExtractIndex(N), body)
 *       )
 *     )
 *   )
 */
/**
 * Constraint for the body callback return type. Only requires the output
 * phantom fields — omits `__phantom_in` and `__in` so that body actions
 * with `In = never` (e.g. pipelines starting from a VarRef) are assignable.
 *
 * This is necessary because `TypedAction<never, X>` is not assignable to
 * `Pipeable<any, X>`: the contravariant `__phantom_in` field check fails
 * since `(input: never) => void` is not assignable to `(input: any) => void`
 * when distributed across the 9-variant Action union.
 */
type BodyResult<TOut> = Action & {
  __phantom_out?: () => TOut;
  __phantom_out_check?: (output: TOut) => void;
};

export function bind<TBindings extends Action[], TOut>(
  bindings: [...TBindings],
  body: (vars: InferVarRefs<TBindings>) => BodyResult<TOut>,
): TypedAction<ExtractInput<TBindings[number]>, TOut> {
  // 1. Gensym one effectId per binding.
  const effectIds = bindings.map(() => allocateEffectId());

  // 2. Create VarRefs (Perform nodes) for each binding.
  const varRefs = effectIds.map((id) => createVarRef(id));

  // 3. Invoke the body callback with the VarRefs.
  const bodyAction = body(varRefs as InferVarRefs<TBindings>) as Action;

  // 4. Build nested Handles from inside out.
  //    Innermost: extract pipeline_input (last All element) → user body
  const pipelineInputIndex = bindings.length;
  let inner: Action = {
    kind: "Chain",
    first: { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "ExtractIndex", value: pipelineInputIndex } } },
    rest: bodyAction,
  };
  for (let i = effectIds.length - 1; i >= 0; i--) {
    inner = {
      kind: "Handle",
      effect_id: effectIds[i],
      handler: readVar(i),
      body: inner,
    };
  }

  // 5. All(...bindings, identity()) → nested Handles
  const allActions = [...bindings.map((b) => b as Action), identity as Action];
  return typedAction({
    kind: "Chain",
    first: { kind: "All", actions: allActions },
    rest: inner,
  });
}

// ---------------------------------------------------------------------------
// bindInput — bind the pipeline input
// ---------------------------------------------------------------------------

/**
 * Convenience wrapper for the common pattern of capturing the pipeline
 * input as a VarRef. The body's pipeline input is `never` — the input
 * is dropped, so the body must access it through the VarRef.
 *
 * Sugar for: `bind([identity()], ([input]) => pipe(drop, body(input)))`
 *
 * TOut defaults to `any` so callers can specify just TIn:
 *   bindInput<FileEntry>((entry) => ...)
 */
export function bindInput<TIn, TOut = any>(
  body: (input: VarRef<TIn>) => BodyResult<TOut>,
): TypedAction<TIn, TOut> {
  return bind([identity], ([input]) =>
    pipe(drop, body(input)),
  );
}
