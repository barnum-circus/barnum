import type { z } from "zod";
import { fileURLToPath } from "node:url";

export interface HandlerDefinition<C = unknown, V = unknown> {
  stepConfigValidator: z.ZodType<C>;
  getStepValueValidator: (stepConfig: C) => z.ZodType<V>;
  handle: (context: HandlerContext<C, V>) => Promise<FollowUpTask[]>;
}

export interface HandlerContext<C, V> {
  stepConfig: C;
  value: V;
  config: unknown;
  stepName: string;
}

export interface FollowUpTask {
  kind: string;
  value: unknown;
}

// ==================== Opaque Handler ====================

const HANDLER_BRAND = Symbol.for("barnum:handler");

export class Handler<C = unknown, V = unknown> {
  readonly [HANDLER_BRAND] = true as const;
  /** @internal */ readonly __filePath: string;
  /** @internal */ readonly __definition: HandlerDefinition<C, V>;

  /** @internal */
  constructor(definition: HandlerDefinition<C, V>, filePath: string) {
    this.__definition = definition;
    this.__filePath = filePath;
  }

  handle(context: HandlerContext<C, V>): Promise<FollowUpTask[]> {
    return this.__definition.handle(context);
  }
}

export function isHandler(x: unknown): x is Handler {
  return typeof x === "object" && x !== null && HANDLER_BRAND in x;
}

function getCallerFilePath(): string {
  const original = Error.prepareStackTrace;
  let callerFile: string | undefined;

  Error.prepareStackTrace = (_err, stack) => {
    // Frame 0: getCallerFilePath
    // Frame 1: createHandler
    // Frame 2: the file that called createHandler
    const frame = stack[2];
    callerFile = frame?.getFileName() ?? undefined;
    return "";
  };

  const err = new Error();
  void err.stack;
  Error.prepareStackTrace = original;

  if (!callerFile) {
    throw new Error(
      "createHandler: could not determine caller file path from stack trace. " +
        "Pass the path explicitly: createHandler(definition, { path: import.meta.filename })",
    );
  }

  if (callerFile.startsWith("file://")) {
    return fileURLToPath(callerFile);
  }
  return callerFile;
}

export function createHandler<C, V>(
  definition: HandlerDefinition<C, V>,
  opts?: { path?: string },
): Handler<C, V> {
  const filePath = opts?.path ?? getCallerFilePath();
  return new Handler(definition, filePath);
}
