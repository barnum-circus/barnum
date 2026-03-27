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

// -- Sequence: type-safe chaining via overloads --

export function sequence<T1, T2>(a1: TypedAction<T1, T2>): TypedAction<T1, T2>;
export function sequence<T1, T2, T3>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
): TypedAction<T1, T3>;
export function sequence<T1, T2, T3, T4>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
): TypedAction<T1, T4>;
export function sequence<T1, T2, T3, T4, T5>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
): TypedAction<T1, T5>;
export function sequence<T1, T2, T3, T4, T5, T6>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
): TypedAction<T1, T6>;
export function sequence<T1, T2, T3, T4, T5, T6, T7>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
): TypedAction<T1, T7>;
export function sequence<T1, T2, T3, T4, T5, T6, T7, T8>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
  a7: TypedAction<T7, T8>,
): TypedAction<T1, T8>;
export function sequence<T1, T2, T3, T4, T5, T6, T7, T8, T9>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
  a7: TypedAction<T7, T8>,
  a8: TypedAction<T8, T9>,
): TypedAction<T1, T9>;
export function sequence<T1, T2, T3, T4, T5, T6, T7, T8, T9, T10>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
  a7: TypedAction<T7, T8>,
  a8: TypedAction<T8, T9>,
  a9: TypedAction<T9, T10>,
): TypedAction<T1, T10>;
export function sequence<T1, T2, T3, T4, T5, T6, T7, T8, T9, T10, T11>(
  a1: TypedAction<T1, T2>,
  a2: TypedAction<T2, T3>,
  a3: TypedAction<T3, T4>,
  a4: TypedAction<T4, T5>,
  a5: TypedAction<T5, T6>,
  a6: TypedAction<T6, T7>,
  a7: TypedAction<T7, T8>,
  a8: TypedAction<T8, T9>,
  a9: TypedAction<T9, T10>,
  a10: TypedAction<T10, T11>,
): TypedAction<T1, T11>;
export function sequence(...actions: TypedAction[]): TypedAction {
  return { kind: "Sequence", actions };
}

// -- Other typed builders --

export function traverse<In, Out>(
  action: TypedAction<In, Out>,
): TypedAction<In[], Out[]> {
  return { kind: "Traverse", action };
}

// -- All: parallel fanout with tuple output --

export function all<In, O1>(a1: TypedAction<In, O1>): TypedAction<In, [O1]>;
export function all<In, O1, O2>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
): TypedAction<In, [O1, O2]>;
export function all<In, O1, O2, O3>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
): TypedAction<In, [O1, O2, O3]>;
export function all<In, O1, O2, O3, O4>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
): TypedAction<In, [O1, O2, O3, O4]>;
export function all<In, O1, O2, O3, O4, O5>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
  a5: TypedAction<In, O5>,
): TypedAction<In, [O1, O2, O3, O4, O5]>;
export function all<In, O1, O2, O3, O4, O5, O6>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
  a5: TypedAction<In, O5>,
  a6: TypedAction<In, O6>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6]>;
export function all<In, O1, O2, O3, O4, O5, O6, O7>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
  a5: TypedAction<In, O5>,
  a6: TypedAction<In, O6>,
  a7: TypedAction<In, O7>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6, O7]>;
export function all<In, O1, O2, O3, O4, O5, O6, O7, O8>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
  a5: TypedAction<In, O5>,
  a6: TypedAction<In, O6>,
  a7: TypedAction<In, O7>,
  a8: TypedAction<In, O8>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6, O7, O8]>;
export function all<In, O1, O2, O3, O4, O5, O6, O7, O8, O9>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
  a5: TypedAction<In, O5>,
  a6: TypedAction<In, O6>,
  a7: TypedAction<In, O7>,
  a8: TypedAction<In, O8>,
  a9: TypedAction<In, O9>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6, O7, O8, O9]>;
export function all<In, O1, O2, O3, O4, O5, O6, O7, O8, O9, O10>(
  a1: TypedAction<In, O1>,
  a2: TypedAction<In, O2>,
  a3: TypedAction<In, O3>,
  a4: TypedAction<In, O4>,
  a5: TypedAction<In, O5>,
  a6: TypedAction<In, O6>,
  a7: TypedAction<In, O7>,
  a8: TypedAction<In, O8>,
  a9: TypedAction<In, O9>,
  a10: TypedAction<In, O10>,
): TypedAction<In, [O1, O2, O3, O4, O5, O6, O7, O8, O9, O10]>;
export function all(...actions: TypedAction[]): TypedAction {
  return { kind: "All", actions };
}

export function matchCases<In, Out>(
  cases: Record<string, TypedAction<In, Out>>,
): TypedAction<In, Out> {
  return { kind: "Match", cases };
}

export function loop<T>(body: TypedAction<T, T>): TypedAction<T, T> {
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
