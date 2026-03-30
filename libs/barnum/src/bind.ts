import { type Action, type ExtractOutput, type Pipeable, type TypedAction, typedAction } from "./ast.js";
import { identity, drop } from "./builtins.js";
import { pipe } from "./pipe.js";

// ---------------------------------------------------------------------------
// Effect ID counter
// ---------------------------------------------------------------------------

let nextEffectId = 0;

/** Reset the effect ID counter. For test isolation only. */
export function resetEffectIdCounter(): void {
  nextEffectId = 0;
}

// ---------------------------------------------------------------------------
// VarRef — typed reference to a bound value
// ---------------------------------------------------------------------------

/**
 * A VarRef is a Perform node wrapped with phantom types. Input is `never`
 * because VarRefs don't consume pipeline input — they raise an effect.
 * Output is `TValue`, the concrete type of the bound value.
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
 */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type InferVarRefs<TBindings extends Pipeable<any, any>[]> = {
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
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function bind<TIn, TBindings extends Pipeable<TIn, any>[], TOut>(
  bindings: [...TBindings],
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  body: (vars: InferVarRefs<TBindings>) => Pipeable<any, TOut>,
): TypedAction<TIn, TOut> {
  // 1. Gensym one effectId per binding.
  const effectIds = bindings.map(() => nextEffectId++);

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
  const allActions = [...bindings.map((b) => b as Action), identity() as Action];
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
 * Sugar for: `bind([identity()], ([input]) => pipe(drop(), body(input)))`
 */
export function bindInput<TIn, TOut>(
  body: (input: VarRef<TIn>) => Pipeable<never, TOut>,
): TypedAction<TIn, TOut> {
  return bind([identity<TIn>()], ([input]) =>
    pipe(drop(), body(input)),
  );
}
