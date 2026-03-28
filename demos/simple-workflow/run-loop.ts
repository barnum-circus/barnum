/**
 * Demo: polling loop workflow.
 *
 * Pipeline: start-polling → loop(poll-status)
 *
 * The poll handler returns Continue until attempt >= 3, then Break.
 *
 * Usage: pnpm exec tsx run-loop.ts
 */

import { execFileSync } from "child_process";
import { createRequire } from "module";
import path from "path";
import { fileURLToPath } from "url";

import { configBuilder, pipe, loop } from "@barnum/barnum/src/ast.js";
import startPolling from "./handlers/start-polling.js";
import pollStatus from "./handlers/poll-status.js";

const workflow = configBuilder()
  .workflow(() => pipe(startPolling(), loop(pollStatus())));

// Resolve tsx executor
const require = createRequire(import.meta.url);
const tsxPath = require.resolve("tsx/cli");
const executor = `node ${tsxPath}`;

// Resolve paths
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const workerPath = path.resolve(__dirname, "../../libs/barnum/src/worker.ts");
const barnumBinary = path.resolve(__dirname, "../../target/debug/barnum");

const configJson = JSON.stringify(workflow);

console.error("=== Running polling loop workflow ===\n");

try {
  execFileSync(barnumBinary, [
    "run",
    "--config", configJson,
    "--executor", executor,
    "--worker", workerPath,
  ], { stdio: ["inherit", "inherit", "inherit"] });
} catch (e: unknown) {
  const err = e as { status?: number };
  process.exit(err.status || 1);
}
