/**
 * Demo: deployment pipeline with all post-deploy checks.
 *
 * Pipeline: initialize → build → deploy → all(check-health, notify, report)
 *
 * Usage: pnpm exec tsx run-all.ts
 */

import { workflowBuilder, pipe, all } from "@barnum/barnum/src/ast.js";
import initialize from "./handlers/initialize.js";
import build from "./handlers/build.js";
import deploy from "./handlers/deploy.js";
import report from "./handlers/report.js";
import checkHealth from "./handlers/check-health.js";
import notify from "./handlers/notify.js";

console.error("=== Running all post-deploy workflow ===\n");

await workflowBuilder()
  .workflow(() =>
    pipe(
      initialize,
      build,
      deploy,
      all(checkHealth, notify, report),
    ),
  )
  .run();
