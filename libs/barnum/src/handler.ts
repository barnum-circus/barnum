import { fileURLToPath } from "node:url";
import type { z } from "zod";
import { type TypedAction, typedAction } from "./ast.js";

// ---------------------------------------------------------------------------
// HandlerDefinition — the user's handle function + optional validators
// ---------------------------------------------------------------------------

export interface HandlerDefinition<
  TValue = unknown,
  TOutput = unknown,
  TStepConfig = unknown,
> {
  inputValidator?: z.ZodType<TValue>;
  stepConfigValidator?: z.ZodType<TStepConfig>;
  handle: (context: {
    value: TValue;
    stepConfig: TStepConfig;
  }) => Promise<TOutput>;
}

/** Runtime-only handler definition shape — erases generic type info. */
interface UntypedHandlerDefinition {
  inputValidator?: z.ZodType;
  stepConfigValidator?: z.ZodType;
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  handle: (...args: any[]) => Promise<unknown>;
}

// ---------------------------------------------------------------------------
// Handler — opaque typed handler reference
// ---------------------------------------------------------------------------

const HANDLER_BRAND = Symbol.for("barnum:handler");

/**
 * Opaque handler reference with typed metadata. The `__definition` property
 * is non-enumerable — invisible to `JSON.stringify`, visible to the worker.
 */
export type Handler<TValue = unknown, TOutput = unknown> = TypedAction<TValue, TOutput> & {
  readonly [HANDLER_BRAND]: true;
  readonly __definition: UntypedHandlerDefinition;
};

// ---------------------------------------------------------------------------
// getCallerFilePath
// ---------------------------------------------------------------------------

/**
 * Deduces the caller's file path from the V8 stack trace API.
 * Frame 0 = getCallerFilePath, Frame 1 = createHandler, Frame 2 = the caller.
 */
function getCallerFilePath(): string {
  const original = Error.prepareStackTrace;
  let callerFile: string | undefined;

  Error.prepareStackTrace = (_err, stack): string => {
    const frame = stack[2];
    callerFile = frame?.getFileName() ?? undefined;
    return "";
  };

  const err = new Error("stack trace capture");
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

// ---------------------------------------------------------------------------
// createHandler — handlers with no config, returns TypedAction directly
// ---------------------------------------------------------------------------

// With inputValidator: handler accepts typed pipeline input.
export function createHandler<TValue, TOutput>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, TOutput>;

// Without inputValidator: handler takes no pipeline input.
export function createHandler<TOutput>(
  definition: {
    handle: () => Promise<TOutput>;
  },
  exportName?: string,
): Handler<never, TOutput>;

// Implementation
export function createHandler(
  definition: UntypedHandlerDefinition,
  exportName?: string,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
): any {
  const filePath = getCallerFilePath();
  const funcName = exportName ?? "default";

  const action = typedAction({
    kind: "Invoke",
    handler: { kind: "TypeScript", module: filePath, func: funcName },
  });

  // Non-enumerable: invisible to JSON.stringify, visible to the worker
  Object.defineProperty(action, HANDLER_BRAND, {
    value: true,
    enumerable: false,
  });
  Object.defineProperty(action, "__definition", {
    value: definition,
    enumerable: false,
  });

  return action;
}

// ---------------------------------------------------------------------------
// createHandlerWithConfig — handlers that need static config
// ---------------------------------------------------------------------------

// With inputValidator: handler accepts typed pipeline input + config.
export function createHandlerWithConfig<TValue, TOutput, TStepConfig>(
  definition: {
    inputValidator: z.ZodType<TValue>;
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { value: TValue; stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, TOutput>;

// Without inputValidator: handler takes no pipeline input, has config.
export function createHandlerWithConfig<TOutput, TStepConfig>(
  definition: {
    stepConfigValidator: z.ZodType<TStepConfig>;
    handle: (context: { stepConfig: TStepConfig }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<never, TOutput>;

// Implementation
export function createHandlerWithConfig(
  definition: UntypedHandlerDefinition,
  exportName?: string,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
): any {
  const filePath = getCallerFilePath();
  const funcName = exportName ?? "default";

  // Internal handle that unpacks the [value, config] tuple from Parallel
  const internalDefinition: UntypedHandlerDefinition = {
    handle: ({ value }: { value: unknown }) => {
      const [pipelineValue, config] = value as [unknown, unknown];
      return definition.handle({ value: pipelineValue, stepConfig: config });
    },
  };

  const invokeAction = typedAction({
    kind: "Invoke",
    handler: { kind: "TypeScript", module: filePath, func: funcName },
  });

  // Non-enumerable: invisible to JSON.stringify, visible to the worker
  Object.defineProperty(invokeAction, HANDLER_BRAND, {
    value: true,
    enumerable: false,
  });
  Object.defineProperty(invokeAction, "__definition", {
    value: internalDefinition,
    enumerable: false,
  });

  return (config: unknown) =>
    typedAction({
      kind: "Chain",
      first: {
        kind: "Parallel",
        actions: [
          { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Identity" } } },
          { kind: "Invoke", handler: { kind: "Builtin", builtin: { kind: "Constant", value: config } } },
        ],
      },
      rest: invokeAction,
    });
}
