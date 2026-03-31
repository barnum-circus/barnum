/**
 * Simple workflow demo: list files and refactor each one.
 *
 * Usage: pnpm exec tsx run.ts
 */

import { workflowBuilder } from "@barnum/barnum/src/ast.js";
import { listFiles, refactor } from "./handlers/steps.js";

await workflowBuilder()
  .workflow(() => listFiles.forEach(refactor).drop())
  .run();
