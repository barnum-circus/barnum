import {
  type Action,
  type ExtractInput,
  type ExtractOutput,
  type TypedAction,
  typedAction,
} from "./ast.js";
import { chain } from "./chain.js";
import { all } from "./all.js";
import { identity, drop, getIndex } from "./builtins.js";
import { allocateResumeHandlerId, type ResumeHandlerId } from "./effect-id.js";
import { pipe } from "./pipe.js";

// ---------------------------------------------------------------------------
// VarRef — typed reference to a bound value
// ---------------------------------------------------------------------------

/**
 * A typed reference to a bound value. Output is `TValue`.
 *
 * Use `.then()` (not `pipe()`) when chaining a VarRef into a generic
 * action like `pick` or `getField` — pipe overloads can't infer
 * the generic's type parameter from the VarRef's output.
 */
export type VarRef<TValue> = TypedAction<any, TValue>;

function createVarRef<TValue>(
  resumeHandlerId: ResumeHandlerId,
): VarRef<TValue> {
  return typedAction({
    kind: "ResumePerform",
    resume_handler_id: resumeHandlerId,
  });
}

// ---------------------------------------------------------------------------
// InferVarRefs — map bindings to VarRef types
// ---------------------------------------------------------------------------

/**
 * Maps each binding's output type to a VarRef. TypeScript resolves
 * ExtractOutput from each binding expression.
 *
 * Constraint is `Action[]` (not `Pipeable<any, any>[]`) so that
 * `ExtractOutput` extracts the correct output type from the phantom
 * fields on the concrete types without fighting invariant `__in` checks.
 */
export type InferVarRefs<TBindings extends Action[]> = {
  [K in keyof TBindings]: VarRef<ExtractOutput<TBindings[K]>>;
};

// ---------------------------------------------------------------------------
// readVar — handler DAG for the nth binding
// ---------------------------------------------------------------------------

/**
 * Returns an action that extracts the nth value from the ResumeHandle's
 * state tuple and passes state through unchanged. When a ResumePerform
 * fires, the engine calls the handler with `[payload, state]`. For bind,
 * `state` (index 1) is the full All output tuple. The handler produces
 * `[state[n], state]` — value is state[n], new_state is state (unchanged).
 *
 * Expanded AST: All(Chain(GetIndex(1), GetIndex(n)), GetIndex(1))
 */
function readVar(n: number): Action {
  return all(
    chain(getIndex(1) as any, getIndex(n)),
    getIndex(1) as any,
  ) as Action;
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
 *     ResumeHandle(r0, readVar(0),
 *       ResumeHandle(r1, readVar(1),
 *         Chain(GetIndex(N), body)
 *       )
 *     )
 *   )
 */
/**
 * Constraint for the body callback return type. Only requires the output
 * phantom fields — omits `__in` and `__in_co` so that body actions with
 * `In = never` (e.g. pipelines starting from a VarRef) are assignable.
 */
type BodyResult<TOut> = Action & {
  __out?: () => TOut;
  __out_contra?: (output: TOut) => void;
};

export function bind<TBindings extends Action[], TOut>(
  bindings: [...TBindings],
  body: (vars: InferVarRefs<TBindings>) => BodyResult<TOut>,
): TypedAction<ExtractInput<TBindings[number]>, TOut> {
  // 1. Gensym one resumeHandlerId per binding.
  const resumeHandlerIds = bindings.map(() => allocateResumeHandlerId());

  // 2. Create VarRefs (ResumePerform nodes) for each binding.
  const varRefs = resumeHandlerIds.map((id) => createVarRef(id));

  // 3. Invoke the body callback with the VarRefs.
  const bodyAction = body(varRefs as InferVarRefs<TBindings>) as Action;

  // 4. Build nested Handles from inside out.
  //    Innermost: extract pipeline_input (last All element) → user body
  const pipelineInputIndex = bindings.length;
  let inner: Action = chain(
    getIndex(pipelineInputIndex) as any,
    bodyAction as any,
  ) as Action;
  for (let i = resumeHandlerIds.length - 1; i >= 0; i--) {
    inner = {
      kind: "ResumeHandle",
      resume_handler_id: resumeHandlerIds[i],
      handler: readVar(i),
      body: inner,
    };
  }

  // 5. All(...bindings, identity()) → nested Handles
  const allAction: Action = {
    kind: "All",
    actions: [...bindings.map((b) => b as Action), identity() as Action],
  };
  return typedAction(chain(allAction as any, inner as any) as Action);
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
  return bind([identity()], ([input]) => pipe(drop, body(input)));
}
