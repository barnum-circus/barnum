/**
 * Simple workflow demo: list files, refactor each, then type-check.
 *
 * Usage: pnpm exec tsx run.ts
 */

import { workflowBuilder, pipe } from "@barnum/barnum/src/ast.js";
import { listFiles, refactor, typeCheckFix } from "./handlers/steps.js";

await workflowBuilder()
  .workflow(() => listFiles.forEach(pipe(refactor, typeCheckFix)).drop())
  .run();
