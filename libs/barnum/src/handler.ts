import { fileURLToPath } from "node:url";
import { type z, z as zod } from "zod";
import { type TypedAction, typedAction } from "./ast.js";
import { zodToCheckedJsonSchema } from "./schema.js";

// ---------------------------------------------------------------------------
// HandlerDefinition — the user's handle function + optional validators
// ---------------------------------------------------------------------------

export interface HandlerDefinition<
  TValue = unknown,
  TOutput = unknown,
  TStepConfig = unknown,
> {
  inputValidator?: z.ZodType<TValue>;
  outputValidator?: z.ZodType<TOutput>;
  stepConfigValidator?: z.ZodType<TStepConfig>;
  handle: (context: {
    value: TValue;
    stepConfig: TStepConfig;
  }) => Promise<TOutput>;
}

/** Runtime-only handler definition shape — erases generic type info. */
interface UntypedHandlerDefinition {
  inputValidator?: z.ZodType;
  outputValidator?: z.ZodType;
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
export type Handler<TValue = unknown, TOutput = unknown> = TypedAction<
  TValue,
  TOutput
> & {
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
// HandlerOutput — maps void → never so fire-and-forget handlers compose
// ---------------------------------------------------------------------------

/**
 * Handlers that return `Promise<void>` produce `never` output. This means
 * they naturally compose in pipes without needing `.drop()` — a handler
 * that returns nothing produces a value no one can observe.
 */
type HandlerOutput<TOutput> = [TOutput] extends [void] ? never : TOutput;

// ---------------------------------------------------------------------------
// createHandler — single overload, validators optional
// ---------------------------------------------------------------------------

export function createHandler<TValue = never, TOutput = unknown>(
  definition: {
    inputValidator?: z.ZodType<TValue>;
    outputValidator?: z.ZodType<NoInfer<TOutput>>;
    handle: (context: { value: TValue }) => Promise<TOutput>;
  },
  exportName?: string,
): Handler<TValue, HandlerOutput<TOutput>>;

// Implementation
export function createHandler(
  definition: UntypedHandlerDefinition,
  exportName?: string,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
): any {
  const filePath = getCallerFilePath();
  const funcName = exportName ?? "default";

  const inputSchema = definition.inputValidator
    ? zodToCheckedJsonSchema(
        definition.inputValidator,
        `${filePath}:${funcName} input`,
      )
    : undefined;
  const outputSchema = definition.outputValidator
    ? zodToCheckedJsonSchema(
        definition.outputValidator,
        `${filePath}:${funcName} output`,
      )
    : undefined;

  const action = typedAction({
    kind: "Invoke",
    handler: {
      kind: "TypeScript",
      module: filePath,
      func: funcName,
      ...(inputSchema && { input_schema: inputSchema }),
      ...(outputSchema && { output_schema: outputSchema }),
    },
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
// createHandlerWithConfig — single overload, validators optional
// ---------------------------------------------------------------------------

export function createHandlerWithConfig<
  TValue = never,
  TOutput = unknown,
  TStepConfig = unknown,
>(
  definition: {
    inputValidator?: z.ZodType<TValue>;
    outputValidator?: z.ZodType<NoInfer<TOutput>>;
    stepConfigValidator?: z.ZodType<TStepConfig>;
    handle: (context: {
      value: TValue;
      stepConfig: TStepConfig;
    }) => Promise<TOutput>;
  },
  exportName?: string,
): (config: TStepConfig) => TypedAction<TValue, HandlerOutput<TOutput>>;

// Implementation
export function createHandlerWithConfig(
  definition: UntypedHandlerDefinition,
  exportName?: string,
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
): any {
  const filePath = getCallerFilePath();
  const funcName = exportName ?? "default";

  // The invoke receives [value, config] from All(Identity, Constant(config)).
  // Build a tuple validator so the Rust engine validates the combined input.
  const inputSchema = zodToCheckedJsonSchema(
    zod.tuple([
      definition.inputValidator ?? zod.unknown(),
      definition.stepConfigValidator ?? zod.unknown(),
    ]),
    `${filePath}:${funcName} input`,
  );
  const outputSchema = definition.outputValidator
    ? zodToCheckedJsonSchema(
        definition.outputValidator,
        `${filePath}:${funcName} output`,
      )
    : undefined;

  // Internal handle that unpacks the [value, config] tuple from All
  const internalDefinition: UntypedHandlerDefinition = {
    handle: ({ value }: { value: unknown }) => {
      const [pipelineValue, config] = value as [unknown, unknown];
      return definition.handle({ value: pipelineValue, stepConfig: config });
    },
  };

  const invokeAction = typedAction({
    kind: "Invoke",
    handler: {
      kind: "TypeScript",
      module: filePath,
      func: funcName,
      input_schema: inputSchema,
      ...(outputSchema && { output_schema: outputSchema }),
    },
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

  // The factory function is the module export, so it must also carry
  // __definition for the worker to find (the worker imports the module
  // and accesses the named export, which is this function).
  const factory = (config: unknown): TypedAction =>
    typedAction({
      kind: "Chain",
      first: {
        kind: "All",
        actions: [
          {
            kind: "Invoke",
            handler: { kind: "Builtin", builtin: { kind: "Identity" } },
          },
          {
            kind: "Invoke",
            handler: {
              kind: "Builtin",
              builtin: { kind: "Constant", value: config },
            },
          },
        ],
      },
      rest: invokeAction,
    });

  Object.defineProperty(factory, HANDLER_BRAND, {
    value: true,
    enumerable: false,
  });
  Object.defineProperty(factory, "__definition", {
    value: internalDefinition,
    enumerable: false,
  });

  return factory;
}
