import { fileURLToPath } from "url";
import type { z } from "zod";
import type { TypedAction } from "./ast.js";

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
 * Opaque handler reference with typed metadata. This is the base type
 * accepted by `call()` — it only needs the handler's identity, not
 * callability.
 */
export type Handler<
  TValue = unknown,
  TOutput = unknown,
  TStepConfig = unknown,
> = {
  readonly [HANDLER_BRAND]: true;
  readonly __filePath: string;
  readonly __exportName: string;
  readonly __definition: HandlerDefinition<TValue, TOutput, TStepConfig>;
  readonly __phantom_in: (input: TValue) => void;
  readonly __phantom_out: () => TOutput;
  readonly __phantom_step_config: TStepConfig;
};

/**
 * A handler that can be invoked directly to produce a TypedAction.
 * Created by `createHandler`.
 *
 * ```ts
 * import setup from "./handlers/setup.js";
 *
 * // Direct invocation (preferred):
 * pipe(setup(), process());
 *
 * // With step config:
 * setup({ stepConfig: { timeout: 5000 } });
 * ```
 */
export type CallableHandler<
  TValue = unknown,
  TOutput = unknown,
  TStepConfig = unknown,
> = ((
  options?: { stepConfig?: TStepConfig },
) => TypedAction<TValue, TOutput>) &
  Handler<TValue, TOutput, TStepConfig>;

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
): CallableHandler<TValue, TOutput, never>;

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
): CallableHandler<never, TOutput, never>;

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
    kind: "Invoke",
    handler: {
      kind: "TypeScript",
      module: filePath,
      func: funcName,
      stepConfigSchema: options?.stepConfig,
    },
  });

  return Object.assign(fn, {
    [HANDLER_BRAND]: true as const,
    __filePath: filePath,
    __exportName: funcName,
    __definition: definition,
  });
}

