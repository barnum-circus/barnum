import type { Handler } from "./handler.js";

// ---------------------------------------------------------------------------
// Serializable Types — mirror the Rust AST in barnum_ast
// ---------------------------------------------------------------------------

export type Action =
  | InvokeAction
  | PipeAction
  | ForEachAction
  | ParallelAction
  | BranchAction
  | LoopAction
  | AttemptAction
  | StepAction;

export type InvokeAction = {
  kind: "Invoke";
  handler: HandlerKind;
};

export type PipeAction = {
  kind: "Pipe";
  actions: Action[];
};

export type ForEachAction = {
  kind: "ForEach";
  action: Action;
};

export type ParallelAction = {
  kind: "Parallel";
  actions: Action[];
};

export type BranchAction = {
  kind: "Branch";
  cases: Record<string, Action>;
};

export type LoopAction = {
  kind: "Loop";
  body: Action;
};

export type AttemptAction = {
  kind: "Attempt";
  action: Action;
};

export type StepAction = {
  kind: "Step";
  step: string;
};

// ---------------------------------------------------------------------------
// HandlerKind
// ---------------------------------------------------------------------------

export type HandlerKind = TypeScriptHandler;

export type TypeScriptHandler = {
  kind: "TypeScript";
  module: string;
  func: string;
  stepConfigSchema?: unknown;
  valueSchema?: unknown;
};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export type Config<Out = any> = {
  workflow: TypedAction<never, Out>;
  steps?: Record<string, Action>;
};

// ---------------------------------------------------------------------------
// Phantom Types — type-safe input/output tracking
// ---------------------------------------------------------------------------

/**
 * An action with tracked input/output types. Phantom fields enforce variance
 * and are never set at runtime — they exist only for the TypeScript compiler.
 *
 * __phantom_in: contravariant — ensures pipe chaining correctness
 *   (output of step N is assignable to input of step N+1)
 * __phantom_out: covariant — tracks output type
 * __in: covariant — enables config() to reject workflows that expect input
 *   (the contravariant phantom makes never the most permissive input,
 *   so a second covariant phantom is needed for the entry point check)
 */
export type TypedAction<In = unknown, Out = unknown> = Action & {
  __phantom_in?: (input: In) => void;
  __phantom_out?: () => Out;
  __in?: In;
};

// ---------------------------------------------------------------------------
// Combinators
// ---------------------------------------------------------------------------

export { pipe } from "./pipe.js";
export { parallel } from "./parallel.js";

export function invoke<TValue, TOutput, TStepConfig>(
  handler: Handler<TValue, TOutput, TStepConfig>,
  ...args: [TStepConfig] extends [never]
    ? []
    : unknown extends TStepConfig
      ? [options?: { stepConfig?: TStepConfig }]
      : [options: { stepConfig: TStepConfig }]
): TypedAction<TValue, TOutput> {
  const options = (args as [{ stepConfig?: TStepConfig }?])[0];
  return {
    kind: "Invoke",
    handler: {
      kind: "TypeScript",
      module: handler.__filePath,
      func: handler.__exportName,
      stepConfigSchema: options?.stepConfig,
    },
  };
}

export function forEach<In, Out>(
  action: TypedAction<In, Out>,
): TypedAction<In[], Out[]> {
  return { kind: "ForEach", action };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function branch<Out>(
  cases: Record<string, TypedAction<any, Out>>,
): TypedAction<{ kind: string }, Out> {
  return { kind: "Branch", cases };
}

export type LoopResult<TContinue, TBreak> =
  | { kind: "Continue"; value: TContinue }
  | { kind: "Break"; value: TBreak };

export function loop<In, Out>(
  body: TypedAction<In, LoopResult<In, Out>>,
): TypedAction<In, Out> {
  return { kind: "Loop", body };
}

export type AttemptResult<T> =
  | { kind: "Ok"; value: T }
  | { kind: "Err"; error: unknown };

export function attempt<In, Out>(
  action: TypedAction<In, Out>,
): TypedAction<In, AttemptResult<Out>> {
  return { kind: "Attempt", action };
}

// ---------------------------------------------------------------------------
// Config builders
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type AnyAction = TypedAction<any, any>;

/** Simple config with no named steps. */
export function config<Out>(workflow: TypedAction<never, Out>): Config<Out> {
  return { workflow };
}

/** Builder for configs with type-safe named steps. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export class ConfigBuilder<TSteps extends Record<string, AnyAction> = {}> {
  private readonly _steps: Record<string, AnyAction>;

  constructor(steps: Record<string, AnyAction> = {}) {
    this._steps = steps;
  }

  /**
   * Register named steps. Accepts either a static object or a function
   * for mutual recursion between steps.
   *
   * Static form:
   * ```ts
   * .registerSteps({ Check: check(), Finalize: finalize() })
   * ```
   *
   * Function form (enables mutual recursion):
   * ```ts
   * .registerSteps((refs) => ({
   *   Writer: pipe(draft(), refs.Reviewer),
   *   Reviewer: pipe(critique(), branch({
   *     Approved: publish(),
   *     Rejected: refs.Writer,
   *   })),
   * }))
   * ```
   */
  registerSteps<NewSteps extends Record<string, AnyAction>>(
    stepsOrFn:
      | NewSteps
      | ((refs: TSteps & Record<string, AnyAction>) => NewSteps),
  ): ConfigBuilder<TSteps & NewSteps> {
    const newSteps =
      typeof stepsOrFn === "function"
        ? stepsOrFn(stepRefProxy<TSteps & Record<string, AnyAction>>())
        : stepsOrFn;
    return new ConfigBuilder({ ...this._steps, ...newSteps });
  }

  /**
   * Define the workflow entry point.
   *
   * @param build - receives step references and a `self` reference for
   *   workflow-level recursion (re-runs the workflow from the top).
   */
  workflow<Out>(
    build: (steps: TSteps, self: AnyAction) => TypedAction<never, Out>,
  ): Config<Out> {
    const stepRefs: Record<string, Action> = {};
    for (const name of Object.keys(this._steps)) {
      stepRefs[name] = { kind: "Step", step: name };
    }
    const self: AnyAction = { kind: "Step", step: "__self__" } as AnyAction;
    const workflow = build(stepRefs as TSteps, self);
    const result: Config<Out> = { workflow };
    if (Object.keys(this._steps).length > 0) {
      result.steps = this._steps;
    }
    return result;
  }
}

/**
 * Creates a Proxy that returns `{ kind: "Step", step: name }` for any
 * property access. Used to provide step references for mutual recursion.
 */
function stepRefProxy<T extends Record<string, AnyAction>>(): T {
  return new Proxy(Object.create(null) as T, {
    get(_target, prop: string | symbol) {
      if (typeof prop === "symbol") return undefined;
      return { kind: "Step", step: prop } as AnyAction;
    },
  });
}

export function configBuilder(): ConfigBuilder {
  return new ConfigBuilder();
}
