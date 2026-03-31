/**
 * Simple workflow demo: list files, refactor each, type-check, fix, and commit.
 *
 * Usage: pnpm exec tsx run.ts
 */

import { workflowBuilder, pipe } from "@barnum/barnum/src/ast.js";
import { listFiles, refactor, typeCheck, fix, commit } from "./handlers/steps.js";

await workflowBuilder()
  .workflow(() => listFiles.forEach(pipe(refactor, typeCheck, fix, commit)).drop())
  .run();
