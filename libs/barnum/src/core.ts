import { fileURLToPath } from "url";
import type { z } from "zod";

// ---------------------------------------------------------------------------
// Serializable Types — mirror the Rust AST in barnum_ast
// ---------------------------------------------------------------------------

export type Action =
  | CallAction
  | SequenceAction
  | TraverseAction
  | AllAction
  | MatchAction
  | LoopAction
  | AttemptAction
  | StepAction;

export type CallAction = {
  kind: "Call";
  handler: HandlerKind;
};

export type SequenceAction = {
  kind: "Sequence";
  actions: Action[];
};

export type TraverseAction = {
  kind: "Traverse";
  action: Action;
};

export type AllAction = {
  kind: "All";
  actions: Action[];
};

export type MatchAction = {
  kind: "Match";
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
  stepConfig?: unknown;
  valueSchema?: unknown;
};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

export type Config = {
  workflow: Action;
  steps?: Record<string, Action>;
};

// ---------------------------------------------------------------------------
// Phantom Types — type-safe input/output tracking
// ---------------------------------------------------------------------------

/**
 * An action with tracked input/output types. Phantom fields enforce variance
 * and are never set at runtime — they exist only for the TypeScript compiler.
 *
 * __phantom_in: contravariant — ensures sequence chaining correctness
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
// Handler — opaque typed handler reference
// ---------------------------------------------------------------------------

export type HandlerDefinition<
  TValue = unknown,
  TOutput = unknown,
  TStepConfig = unknown,
> = {
  stepValueValidator?: z.ZodType<TValue>;
  stepConfigValidator?: z.ZodType<TStepConfig>;
  handle: (context: {
    value: TValue;
    stepConfig: TStepConfig;
  }) => Promise<TOutput>;
};

/** Runtime-only handler definition shape — erases generic type info. */
type UntypedHandlerDefinition = {
  stepValueValidator?: z.ZodType;
  stepConfigValidator?: z.ZodType;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  handle: (...args: any[]) => Promise<unknown>;
};

const HANDLER_BRAND = Symbol.for("barnum:handler");

/**
 * A handler that can be invoked directly to produce a TypedAction, or passed
 * to `call()` for explicit invocation. Created by `createHandler`.
 *
 * ```ts
 * import setup from "./handlers/setup.js";
 *
 * // Direct invocation (preferred):
 * sequence(setup(), process_());
 *
 * // With step config:
 * setup({ stepConfig: { timeout: 5000 } });
 *
 * // Explicit call() still works:
 * call(setup);
 * ```
 */
export type CallableHandler<
  TValue = unknown,
  TOutput = unknown,
  TStepConfig = unknown,
> = ((
  options?: { stepConfig?: TStepConfig },
) => TypedAction<TValue, TOutput>) & {
  readonly [HANDLER_BRAND]: true;
  readonly __filePath: string;
  readonly __exportName: string;
  readonly __definition: HandlerDefinition<TValue, TOutput, TStepConfig>;
  readonly __phantom_in: (input: TValue) => void;
  readonly __phantom_out: () => TOutput;
  readonly __phantom_step_config: TStepConfig;
};

export function isHandler(x: unknown): x is CallableHandler {
  return typeof x === "function" && HANDLER_BRAND in x;
}

/**
 * Deduces the caller's file path from the V8 stack trace API.
 * Frame 0 = getCallerFilePath, Frame 1 = createHandler, Frame 2 = the caller.
 */
function getCallerFilePath(): string {
  const original = Error.prepareStackTrace;
  let callerFile: string | undefined;

  Error.prepareStackTrace = (_err, stack) => {
    const frame = stack[2];
    callerFile = frame?.getFileName() ?? undefined;
    return "";
  };

  const err = new Error();
  void err.stack;
  Error.prepareStackTrace = original;

  if (!callerFile) {
    throw new Error(
      "createHandler: could not determine caller file path from stack trace.",
    );
  }

  if (callerFile.startsWith("file://")) {
    return fileURLToPath(callerFile);
  }
  return callerFile;
}

// Both validators: handler accepts typed input and step config.
export function createHandler<TValue, TOutput, TStepConfig>(
  definition: {
    stepValueValidator: z.ZodType<TValue>;
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: {
      value: TValue;
      stepConfig: TStepConfig;
    }) => Promise<TOutput>;
  },
  exportName?: string,
): CallableHandler<TValue, TOutput, TStepConfig>;

// Value validator only: handler accepts typed input, no step config.
export function createHandler<TValue, TOutput>(
  definition: {
    stepValueValidator: z.ZodType<TValue>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): CallableHandler<TValue, TOutput, unknown>;

// Config validator only: handler takes no pipeline input, has step config.
export function createHandler<TOutput, TStepConfig>(
  definition: {
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): CallableHandler<never, TOutput, TStepConfig>;

// Neither validator: handler takes no input and no config.
export function createHandler<TOutput>(
  definition: {
    handle: () => Promise<TOutput>;
  },
  exportName?: string,
): CallableHandler<never, TOutput, unknown>;

// Implementation: return type is intentionally broad. The overload signatures
// provide all type safety for callers. TypeScript's overload compatibility
// check cannot reconcile contravariant phantom types across return types
// that differ in their TValue (generic vs never), so the implementation
// uses the erased definition type and returns the unparameterized handler.
export function createHandler(
  definition: UntypedHandlerDefinition,
  exportName?: string,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
): any {
  const filePath = getCallerFilePath();
  const funcName = exportName ?? "default";

  const fn = (options?: { stepConfig?: unknown }): TypedAction => ({
    kind: "Call",
    handler: {
      kind: "TypeScript",
      module: filePath,
      func: funcName,
      stepConfig: options?.stepConfig,
    },
  });

  return Object.assign(fn, {
    [HANDLER_BRAND]: true as const,
    __filePath: filePath,
    __exportName: funcName,
    __definition: definition,
  });
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

export function call<TValue, TOutput, TStepConfig>(
  handler: CallableHandler<TValue, TOutput, TStepConfig>,
  ...args: unknown extends TStepConfig
    ? [options?: { stepConfig?: TStepConfig }]
    : [options: { stepConfig: TStepConfig }]
): TypedAction<TValue, TOutput> {
  const options = args[0];
  return {
    kind: "Call",
    handler: {
      kind: "TypeScript",
      module: handler.__filePath,
      func: handler.__exportName,
      stepConfig: options?.stepConfig,
    },
  };
}

export { sequence } from "./sequence.js";
export { all } from "./all.js";

export function traverse<In, Out>(
  action: TypedAction<In, Out>,
): TypedAction<In[], Out[]> {
  return { kind: "Traverse", action };
}

// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function matchCases<Out>(
  cases: Record<string, TypedAction<any, Out>>,
): TypedAction<{ kind: string }, Out> {
  return { kind: "Match", cases };
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
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export function config(workflow: TypedAction<never, any>): Config {
  return { workflow };
}

/** Builder for configs with type-safe named steps. */
// eslint-disable-next-line @typescript-eslint/no-explicit-any
export class ConfigBuilder<TSteps extends Record<string, AnyAction> = {}> {
  private readonly _steps: Record<string, AnyAction>;

  constructor(steps: Record<string, AnyAction> = {}) {
    this._steps = steps;
  }

  registerSteps<NewSteps extends Record<string, AnyAction>>(
    steps: NewSteps,
  ): ConfigBuilder<TSteps & NewSteps> {
    return new ConfigBuilder({ ...this._steps, ...steps });
  }

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  workflow(build: (steps: TSteps) => TypedAction<never, any>): Config {
    const stepRefs: Record<string, Action> = {};
    for (const name of Object.keys(this._steps)) {
      stepRefs[name] = { kind: "Step", step: name };
    }
    const workflow = build(stepRefs as TSteps);
    const result: Config = { workflow };
    if (Object.keys(this._steps).length > 0) {
      result.steps = this._steps;
    }
    return result;
  }
}

export function configBuilder(): ConfigBuilder {
  return new ConfigBuilder();
}
