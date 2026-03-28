/**
 * Workflow execution: resolves the barnum binary, tsx executor, and worker
 * script, then spawns the workflow as a subprocess.
 */

import { execFileSync } from "child_process";
import { createRequire } from "module";
import path from "path";
import { fileURLToPath } from "url";
import type { Config } from "./ast.js";

const __dirname = path.dirname(fileURLToPath(import.meta.url));

/** Resolve the tsx executor from the caller's node_modules. */
function resolveExecutor(): string {
  const callerRequire = createRequire(process.argv[1] || import.meta.url);
  const tsxPath = callerRequire.resolve("tsx/cli");
  return `node ${tsxPath}`;
}

/** Resolve the barnum binary. BARNUM env var overrides for local dev. */
function resolveBinary(): string {
  if (process.env.BARNUM) {
    return process.env.BARNUM;
  }
  // Default: target/debug/barnum relative to repo root
  // (libs/barnum/src/run.ts → ../../.. → repo root)
  return path.resolve(__dirname, "../../../target/debug/barnum");
}

/** Resolve worker.ts relative to this package. */
function resolveWorker(): string {
  return path.resolve(__dirname, "worker.ts");
}

/** Run a workflow config to completion. Prints result to stdout. */
export function run(config: Config): void {
  const binary = resolveBinary();
  const executor = resolveExecutor();
  const worker = resolveWorker();
  const configJson = JSON.stringify(config);

  execFileSync(binary, [
    "run",
    "--config", configJson,
    "--executor", executor,
    "--worker", worker,
  ], { stdio: "inherit" });
}
