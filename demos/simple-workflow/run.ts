/**
 * Demo: run a simple deployment pipeline workflow.
 *
 * Pipeline: initialize → build → deploy → report
 *
 * Usage: pnpm exec tsx run.ts
 */

import { execFileSync } from "child_process";
import { createRequire } from "module";
import path from "path";
import { fileURLToPath } from "url";

import { pipe } from "@barnum/barnum/src/ast.js";
import initialize from "./handlers/initialize.js";
import build from "./handlers/build.js";
import deploy from "./handlers/deploy.js";
import report from "./handlers/report.js";

// Build the workflow config
const config = {
  workflow: pipe(initialize(), build(), deploy(), report()),
};

// Resolve tsx executor
const require = createRequire(import.meta.url);
const tsxPath = require.resolve("tsx/cli");
const executor = `node ${tsxPath}`;

// Resolve paths
const __dirname = path.dirname(fileURLToPath(import.meta.url));
const workerPath = path.resolve(__dirname, "../../libs/barnum/src/worker.ts");
const barnumBinary = path.resolve(__dirname, "../../target/debug/barnum");

// Serialize config to JSON
const configJson = JSON.stringify(config);

console.error("=== Running deployment pipeline workflow ===");
console.error(`Config: ${configJson}\n`);

// Run the workflow via the Rust CLI
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
