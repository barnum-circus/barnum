import { spawn, type ChildProcess } from "node:child_process";
import { chmodSync } from "node:fs";
import { createRequire } from "node:module";
import type { configFileSchema } from "./barnum-config-schema.zod.js";
import type { z } from "zod";

const require = createRequire(import.meta.url);
const binaryPath: string = process.env.BARNUM ?? require("./index.cjs");

function spawnBarnum(args: string[]): ChildProcess {
  try {
    chmodSync(binaryPath, 0o755);
  } catch {}
  return spawn(binaryPath, args, { stdio: "inherit" });
}

export interface RunOptions {
  pool?: string;
  entrypointValue?: string;
  root?: string;
  logLevel?: string;
  logFile?: string;
  stateLog?: string;
  wake?: string;
}

export class BarnumConfig {
  private readonly config: z.input<typeof configFileSchema>;

  private constructor(config: z.input<typeof configFileSchema>) {
    this.config = config;
  }

  static fromConfig(config: z.input<typeof configFileSchema>): BarnumConfig {
    return new BarnumConfig(config);
  }

  run(opts?: RunOptions): ChildProcess {
    const args = ["run", "--config", JSON.stringify(this.config)];
    if (opts?.pool) args.push("--pool", opts.pool);
    if (opts?.entrypointValue) args.push("--entrypoint-value", opts.entrypointValue);
    if (opts?.root) args.push("--root", opts.root);
    if (opts?.logLevel) args.push("--log-level", opts.logLevel);
    if (opts?.logFile) args.push("--log-file", opts.logFile);
    if (opts?.stateLog) args.push("--state-log", opts.stateLog);
    if (opts?.wake) args.push("--wake", opts.wake);
    return spawnBarnum(args);
  }
}
