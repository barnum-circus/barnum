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
  step: StepRef;
};

export type StepRef = { kind: "Named"; name: string } | { kind: "Root" };

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
 * Refs: tracks step reference names through combinators for compile-time
 *   validation in registerSteps (see ValidateStepRefs)
 */
export type TypedAction<
  In = unknown,
  Out = unknown,
  Refs extends string = never,
> = Action & {
  __phantom_in?: (input: In) => void;
  __phantom_out?: () => Out;
  __in?: In;
  __refs?: { _brand: Refs };
};

// ---------------------------------------------------------------------------
// Type extraction utilities
// ---------------------------------------------------------------------------

/**
 * Extract the input type from a TypedAction.
 *
 * Uses direct phantom field extraction (not full TypedAction matching) to
 * avoid the `TypedAction<any, any, any>` constraint which fails for In=never
 * due to __phantom_in contravariance.
 */
export type ExtractInput<T> =
  T extends { __phantom_in?: (input: infer In) => void } ? In : never;

/**
 * Extract the output type from a TypedAction.
 *
 * Uses direct phantom field extraction to avoid constraint issues.
 */
export type ExtractOutput<T> =
  T extends { __phantom_out?: () => infer Out } ? Out : never;

/**
 * Extract step reference names tracked in a TypedAction's Refs parameter.
 *
 * Uses direct __refs extraction (not full TypedAction matching) to avoid
 * variance issues with contravariant __phantom_in when In = never.
 */
export type ExtractRefs<T> =
  T extends { __refs?: { _brand: infer R } }
    ? R extends string
      ? R
      : never
    : never;

/**
 * Validates that all step references in R resolve to known step names.
 * Known names = keys of R (current batch) + keys of TSteps (previously registered).
 *
 * When valid: resolves to {} (transparent intersection).
 * When invalid: resolves to a type with __error that causes a compile error.
 */
export type ValidateStepRefs<
  R extends Record<string, Action>,
  TSteps extends Record<string, Action> = {},
> = [ExtractRefs<R[keyof R]>] extends [keyof R | keyof TSteps]
  ? // eslint-disable-next-line @typescript-eslint/no-empty-object-type
    {}
  : {
      __error: `Step reference to undefined step: ${Exclude<ExtractRefs<R[keyof R]>, keyof R | keyof TSteps> & string}`;
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

export function forEach<In, Out, R extends string = never>(
  action: TypedAction<In, Out, R>,
): TypedAction<In[], Out[], R> {
  return { kind: "ForEach", action } as TypedAction<In[], Out[], R>;
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function branch<Out, R extends string = never>(
  cases: Record<string, TypedAction<any, Out, R>>,
): TypedAction<{ kind: string }, Out, R> {
  return { kind: "Branch", cases } as TypedAction<{ kind: string }, Out, R>;
}

export type LoopResult<TContinue, TBreak> =
  | { kind: "Continue"; value: TContinue }
  | { kind: "Break"; value: TBreak };

export function loop<In, Out, R extends string = never>(
  body: TypedAction<In, LoopResult<In, Out>, R>,
): TypedAction<In, Out, R> {
  return { kind: "Loop", body } as TypedAction<In, Out, R>;
}

export type AttemptResult<T> =
  | { kind: "Ok"; value: T }
  | { kind: "Err"; error: unknown };

export function attempt<In, Out, R extends string = never>(
  action: TypedAction<In, Out, R>,
): TypedAction<In, AttemptResult<Out>, R> {
  return { kind: "Attempt", action } as TypedAction<In, AttemptResult<Out>, R>;
}

/**
 * Create a typed step reference. The name is tracked at the type level
 * via the Refs parameter so registerSteps can validate all references resolve.
 *
 * Returns TypedAction<any, any, N> — a universal connector since the actual
 * types depend on the referenced step's definition (which may not be fully
 * typed during mutual recursion).
 */
export function stepRef<N extends string>(name: N): TypedAction<any, any, N> {
  return {
    kind: "Step",
    step: { kind: "Named", name },
  } as AnyAction;
}

// ---------------------------------------------------------------------------
// Config builders
// ---------------------------------------------------------------------------

// eslint-disable-next-line @typescript-eslint/no-explicit-any
type AnyAction = TypedAction<any, any, any>;

/**
 * Strip the Refs parameter from registered step types. Refs are a compile-time
 * mechanism for validating step references in registerSteps — once validated,
 * they shouldn't propagate into the workflow callback's return type.
 */
type StripRefs<TSteps> = {
  [K in keyof TSteps]: TypedAction<ExtractInput<TSteps[K]>, ExtractOutput<TSteps[K]>>;
};

/** Simple config with no named steps. */
export function config<Out>(workflow: TypedAction<never, Out>): Config<Out> {
  return { workflow };
}

/** Builder for configs with type-safe named steps. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export class ConfigBuilder<TSteps extends Record<string, AnyAction> = {}> {
  private readonly _steps: Record<string, Action>;

  constructor(steps: Record<string, Action> = {}) {
    this._steps = steps;
  }

  /**
   * Register named steps. Use `stepRef("B")` to create cross-references
   * between steps — all references are validated at compile time.
   *
   * ```ts
   * .registerSteps({
   *   A: pipe(check(), stepRef("B")),
   *   B: pipe(check(), stepRef("A")),
   * })
   * ```
   *
   * References to previously registered steps are also valid:
   * ```ts
   * .registerSteps({ Setup: setup() })
   * .registerSteps({ Pipeline: pipe(stepRef("Setup"), process()) })
   * ```
   */
  registerSteps<R extends Record<string, Action>>(
    steps: R & ValidateStepRefs<R, TSteps>,
  ): ConfigBuilder<TSteps & R> {
    return new ConfigBuilder({
      ...this._steps,
      ...steps,
    }) as ConfigBuilder<TSteps & R>;
  }

  /**
   * Define the workflow entry point.
   *
   * @param build - receives step references and a `self` reference.
   *   `self` is `TypedAction<never, never>` — a jump to the workflow
   *   root. Input `never` because it doesn't consume pipeline data.
   *   Output `never` because the execution path restarts (and `never`
   *   is eliminated from unions, so branches with `self` don't pollute
   *   the output type).
   *
   *   Use `pipe(drop(), self)` to place `self` in a branch case.
   *
   *   Note: ideally `self` would be `TypedAction<never, Out>` so it
   *   carries the workflow's output type, but TypeScript can't infer
   *   a generic from a callback's return and use it in the same
   *   callback's parameter — Out falls back to `unknown`.
   */
  workflow<Out>(
    build: (
      steps: StripRefs<TSteps>,
      self: TypedAction<never, never>,
    ) => TypedAction<never, Out>,
  ): Config<Out> {
    const stepRefs: Record<string, Action> = {};
    for (const name of Object.keys(this._steps)) {
      stepRefs[name] = { kind: "Step", step: { kind: "Named", name } };
    }
    const self = {
      kind: "Step",
      step: { kind: "Root" },
    } as TypedAction<never, never>;
    const workflow = build(stepRefs as StripRefs<TSteps>, self);
    const result: Config<Out> = { workflow };
    if (Object.keys(this._steps).length > 0) {
      result.steps = this._steps;
    }
    return result;
  }
}

export function configBuilder(): ConfigBuilder {
  return new ConfigBuilder();
}
