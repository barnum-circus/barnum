/**
 * Demo: deployment pipeline with parallel post-deploy checks.
 *
 * Pipeline: initialize → build → deploy → parallel(check-health, notify, report)
 *
 * Usage: pnpm exec tsx run-parallel.ts
 */

import { configBuilder, pipe, parallel } from "@barnum/barnum/src/ast.js";
import initialize from "./handlers/initialize.js";
import build from "./handlers/build.js";
import deploy from "./handlers/deploy.js";
import report from "./handlers/report.js";
import checkHealth from "./handlers/check-health.js";
import notify from "./handlers/notify.js";

console.error("=== Running parallel post-deploy workflow ===\n");

await configBuilder()
  .workflow(() =>
    pipe(
      initialize(),
      build(),
      deploy(),
      parallel(checkHealth(), notify(), report()),
    ),
  )
  .run();
