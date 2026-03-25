import { spawn, type ChildProcess } from "node:child_process";
import { chmodSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { configSchema } from "./barnum-config-schema.zod.js";
import { z } from "zod";
import { zodToJsonSchema } from "zod-to-json-schema";

const __dirname = dirname(fileURLToPath(import.meta.url));
const require = createRequire(import.meta.url);
const binaryPath: string = process.env.BARNUM ?? require("./index.cjs");

function resolveExecutor(): string {
  // @ts-expect-error Bun global
  if (typeof Bun !== "undefined") {
    return process.execPath;
  }
  // Resolve tsx from the calling script's node_modules
  const callerRequire = createRequire(process.argv[1] || import.meta.url);
  const tsxPath = callerRequire.resolve("tsx/cli");
  return `node ${tsxPath}`;
}

const runHandlerPath = resolve(__dirname, "actions", "run-handler.ts");

function spawnBarnum(args: string[], cwd?: string): ChildProcess {
  try {
    chmodSync(binaryPath, 0o755);
  } catch {}
  return spawn(binaryPath, args, { stdio: "inherit", cwd });
}

export interface RunOptions {
  entrypointValue?: unknown;
  resumeFrom?: string;
  logLevel?: string;
  logFile?: string;
  stateLog?: string;
  wake?: string;
  cwd?: string;
}

// ==================== Zod subset validation ====================

const UNSUPPORTED_ZOD_TYPES = new Set([
  "ZodEffects", // .transform(), .refine(), .superRefine(), .preprocess()
  "ZodPipeline", // .pipe()
  "ZodBranded", // .brand()
]);

function assertSerializableZod(schema: z.ZodType, stepName: string): void {
  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  const def = (schema as any)._def;
  if (!def) return;

  const typeName: string | undefined = def.typeName;

  if (typeName && UNSUPPORTED_ZOD_TYPES.has(typeName)) {
    throw new Error(
      `Step "${stepName}": Zod schema uses unsupported type "${typeName}". ` +
        `Only JSON-Schema-representable types are allowed. ` +
        `Remove .transform(), .refine(), .preprocess(), .pipe(), or .brand().`,
    );
  }

  // Recurse into compound types
  if (def.innerType) assertSerializableZod(def.innerType, stepName);
  if (def.schema) assertSerializableZod(def.schema, stepName);
  if (def.left) assertSerializableZod(def.left, stepName);
  if (def.right) assertSerializableZod(def.right, stepName);

  // z.object() — check each value
  if (def.shape) {
    const shape = typeof def.shape === "function" ? def.shape() : def.shape;
    for (const value of Object.values(shape)) {
      assertSerializableZod(value as z.ZodType, stepName);
    }
  }

  // z.array(), z.set()
  if (def.type) assertSerializableZod(def.type, stepName);

  // z.union(), z.discriminatedUnion()
  if (def.options) {
    for (const option of def.options) {
      assertSerializableZod(option as z.ZodType, stepName);
    }
  }

  // z.tuple()
  if (def.items) {
    for (const item of def.items) {
      assertSerializableZod(item as z.ZodType, stepName);
    }
  }

  // z.record()
  if (def.keyType) assertSerializableZod(def.keyType, stepName);
  if (def.valueType) assertSerializableZod(def.valueType, stepName);
}

// ==================== BarnumConfig ====================

export class BarnumConfig {
  private readonly config: z.output<typeof configSchema>;

  private constructor(config: z.output<typeof configSchema>) {
    this.config = config;
  }

  static fromConfig(config: z.input<typeof configSchema>): BarnumConfig {
    return new BarnumConfig(configSchema.parse(config));
  }

  private async resolveConfig(): Promise<z.output<typeof configSchema>> {
    const config = structuredClone(this.config);

    for (const step of config.steps) {
      if (step.action.kind !== "TypeScript") continue;
      const action = step.action;

      // Import the handler module
      const mod = await import(action.path);
      const handler = mod[action.exportedAs ?? "default"];

      if (!handler?.stepConfigValidator || !handler?.getStepValueValidator) {
        throw new Error(
          `Step "${step.name}": handler at "${action.path}" is missing required ` +
            `"stepConfigValidator" or "getStepValueValidator". ` +
            `See HandlerDefinition interface.`,
        );
      }

      // Validate step config
      const parsedStepConfig = handler.stepConfigValidator.parse(
        action.stepConfig ?? {},
      );

      // Get value validator
      const valueValidator = handler.getStepValueValidator(parsedStepConfig);

      // Reject non-serializable Zod features
      assertSerializableZod(valueValidator, step.name);

      // Convert Zod → JSON Schema and embed in config
      action.valueSchema = zodToJsonSchema(valueValidator, {
        target: "jsonSchema7",
      });
    }

    return config;
  }

  async run(opts?: RunOptions): Promise<ChildProcess> {
    const config = await this.resolveConfig();
    const args = opts?.resumeFrom
      ? ["run", "--resume-from", opts.resumeFrom]
      : ["run", "--config", JSON.stringify(config)];
    if (opts?.entrypointValue != null)
      args.push("--entrypoint-value", JSON.stringify(opts.entrypointValue));
    if (opts?.logLevel) args.push("--log-level", opts.logLevel);
    if (opts?.logFile) args.push("--log-file", opts.logFile);
    if (opts?.stateLog) args.push("--state-log", opts.stateLog);
    if (opts?.wake) args.push("--wake", opts.wake);
    args.push("--executor", resolveExecutor());
    args.push("--run-handler-path", runHandlerPath);
    return spawnBarnum(args, opts?.cwd);
  }
}
