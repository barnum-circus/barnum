import { spawn, type ChildProcess } from "node:child_process";
import { chmodSync } from "node:fs";
import { createRequire } from "node:module";
import { configSchema } from "./barnum-config-schema.zod.js";
import type { z } from "zod";

const require = createRequire(import.meta.url);
const binaryPath: string = process.env.BARNUM ?? require("./index.cjs");

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
    return spawnBarnum(args, opts?.cwd);
  }
}
