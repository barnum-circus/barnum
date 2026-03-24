import { spawn, type ChildProcess } from "node:child_process";
import { chmodSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { configSchema } from "./barnum-config-schema.zod.js";
import type { z } from "zod";

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
  entrypointValue?: string;
  resumeFrom?: string;
  logLevel?: string;
  logFile?: string;
  stateLog?: string;
  wake?: string;
  cwd?: string;
}

export class BarnumConfig {
  private readonly config: z.output<typeof configSchema>;

  private constructor(config: z.output<typeof configSchema>) {
    this.config = config;
  }

  static fromConfig(config: z.input<typeof configSchema>): BarnumConfig {
    return new BarnumConfig(configSchema.parse(config));
  }

  run(opts?: RunOptions): ChildProcess {
    const args = opts?.resumeFrom
      ? ["run", "--resume-from", opts.resumeFrom]
      : ["run", "--config", JSON.stringify(this.config)];
    if (opts?.entrypointValue) args.push("--entrypoint-value", opts.entrypointValue);
    if (opts?.logLevel) args.push("--log-level", opts.logLevel);
    if (opts?.logFile) args.push("--log-file", opts.logFile);
    if (opts?.stateLog) args.push("--state-log", opts.stateLog);
    if (opts?.wake) args.push("--wake", opts.wake);
    args.push("--executor", resolveExecutor());
    args.push("--run-handler-path", runHandlerPath);
    return spawnBarnum(args, opts?.cwd);
  }
}
