/**
 * Demo: deployment pipeline with parallel post-deploy checks.
 *
 * Pipeline: initialize → build → deploy → parallel(check-health, notify, report)
 *
 * Usage: pnpm exec tsx run-parallel.ts
 */

import { execFileSync } from "child_process";
import { createRequire } from "module";
import path from "path";
import { fileURLToPath } from "url";

import { configBuilder, pipe, parallel } from "@barnum/barnum/src/ast.js";
import initialize from "./handlers/initialize.js";
import build from "./handlers/build.js";
import deploy from "./handlers/deploy.js";
import report from "./handlers/report.js";
import checkHealth from "./handlers/check-health.js";
import notify from "./handlers/notify.js";

const workflow = configBuilder()
  .workflow(() =>
    pipe(
      initialize(),
      build(),
      deploy(),
      parallel(checkHealth(), notify(), report()),
    ),
  );

// Resolve tsx executor
const require = createRequire(import.meta.url);
const tsxPath = require.resolve("tsx/cli");
const executor = `node ${tsxPath}`;

// Resolve paths
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const workerPath = path.resolve(__dirname, "../../libs/barnum/src/worker.ts");
const barnumBinary = path.resolve(__dirname, "../../target/debug/barnum");

const configJson = JSON.stringify(workflow);

console.error("=== Running parallel post-deploy workflow ===\n");

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
