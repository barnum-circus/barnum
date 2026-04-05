import {
  type Action,
  type ExtractInput,
  type ExtractOutput,
  type TypedAction,
  typedAction,
} from "./ast.js";
import { identity, drop } from "./builtins.js";
import { allocateResumeHandlerId, type ResumeHandlerId } from "./effect-id.js";
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
 * Returns an action that extracts the nth value from the ResumeHandle's
 * state tuple and passes state through unchanged. When a ResumePerform
 * fires, the engine calls the handler with `[payload, state]`. For bind,
 * `state` (index 1) is the full All output tuple. The handler produces
 * `[state[n], state]` — value is state[n], new_state is state (unchanged).
 *
 * Expanded AST: All(Chain(ExtractIndex(1), ExtractIndex(n)), ExtractIndex(1))
 */
function readVar(n: number): Action {
  return {
    kind: "All",
    actions: [
      {
        kind: "Chain",
        first: {
          kind: "Invoke",
          handler: {
            kind: "Builtin",
            builtin: { kind: "ExtractIndex", value: 1 },
          },
        },
        rest: {
          kind: "Invoke",
          handler: {
            kind: "Builtin",
            builtin: { kind: "ExtractIndex", value: n },
          },
        },
      },
      {
        kind: "Invoke",
        handler: {
          kind: "Builtin",
          builtin: { kind: "ExtractIndex", value: 1 },
        },
      },
    ],
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
 *     ResumeHandle(r0, readVar(0),
 *       ResumeHandle(r1, readVar(1),
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
  // 1. Gensym one resumeHandlerId per binding.
  const resumeHandlerIds = bindings.map(() => allocateResumeHandlerId());

  // 2. Create VarRefs (ResumePerform nodes) for each binding.
  const varRefs = resumeHandlerIds.map((id) => createVarRef(id));

  // 3. Invoke the body callback with the VarRefs.
  const bodyAction = body(varRefs as InferVarRefs<TBindings>) as Action;

  // 4. Build nested Handles from inside out.
  //    Innermost: extract pipeline_input (last All element) → user body
  const pipelineInputIndex = bindings.length;
  let inner: Action = {
    kind: "Chain",
    first: {
      kind: "Invoke",
      handler: {
        kind: "Builtin",
        builtin: { kind: "ExtractIndex", value: pipelineInputIndex },
      },
    },
    rest: bodyAction,
  };
  for (let i = resumeHandlerIds.length - 1; i >= 0; i--) {
    inner = {
      kind: "ResumeHandle",
      resume_handler_id: resumeHandlerIds[i],
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
  return bind([identity], ([input]) => pipe(drop, body(input)));
}
