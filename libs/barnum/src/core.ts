import { fileURLToPath } from "url";

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
 * An action with tracked input/output types. The phantom fields use function
 * types to enforce correct variance (contravariant input, covariant output)
 * and are never set at runtime — they exist only for the TypeScript compiler.
 */
export type TypedAction<In = unknown, Out = unknown> = Action & {
  __phantom_in?: (input: In) => void;
  __phantom_out?: () => Out;
};

// ---------------------------------------------------------------------------
// Handler — opaque typed handler reference
// ---------------------------------------------------------------------------

export type HandlerDefinition<In = unknown, Out = unknown> = {
  handle: (input: In) => Promise<Out>;
};

const HANDLER_BRAND = Symbol.for("barnum:handler");

export class Handler<In = unknown, Out = unknown> {
  readonly [HANDLER_BRAND] = true;
  readonly __filePath: string;
  readonly __definition: HandlerDefinition<In, Out>;

  // Phantom types — `declare` means these exist only at the type level.
  declare readonly __phantom_in: (input: In) => void;
  declare readonly __phantom_out: () => Out;

  constructor(definition: HandlerDefinition<In, Out>, filePath: string) {
    this.__filePath = filePath;
    this.__definition = definition;
  }
}

export function isHandler(x: unknown): x is Handler {
  return typeof x === "object" && x !== null && HANDLER_BRAND in x;
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

export function createHandler<In, Out>(
  definition: HandlerDefinition<In, Out>,
): Handler<In, Out> {
  const filePath = getCallerFilePath();
  return new Handler(definition, filePath);
}

// ---------------------------------------------------------------------------
// Builders
// ---------------------------------------------------------------------------

export function call<In, Out>(handler: Handler<In, Out>): TypedAction<In, Out> {
  return {
    kind: "Call",
    handler: {
      kind: "TypeScript",
      module: handler.__filePath,
      func: "default",
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

export function matchCases<In, Out>(
  cases: Record<string, TypedAction<In, Out>>,
): TypedAction<In, Out> {
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
export function config(workflow: AnyAction): Config {
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

  workflow(build: (steps: TSteps) => AnyAction): Config {
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
