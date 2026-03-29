/**
 * Demo: run a simple deployment pipeline workflow.
 *
 * Pipeline: initialize → build → deploy → report
 *
 * Usage: pnpm exec tsx run.ts
 */

import { workflowBuilder, pipe } from "@barnum/barnum/src/ast.js";
import initialize from "./handlers/initialize.js";
import build from "./handlers/build.js";
import deploy from "./handlers/deploy.js";
import report from "./handlers/report.js";

console.error("=== Running deployment pipeline workflow ===\n");

await workflowBuilder()
  .workflow(() => pipe(initialize, build, deploy, report))
  .run();
