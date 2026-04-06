/**
 * Simple workflow demo: list files, refactor, type-check, fix, commit, and PR.
 *
 * Usage: pnpm exec tsx run.ts
 */

import { runPipeline, pipe } from "@barnum/barnum";
import {
  listFiles,
  implementRefactor,
  typeCheckFiles,
  fixTypeErrors,
  commitChanges,
  createPullRequest,
} from "./handlers/steps.js";

runPipeline(
  listFiles.forEach(
    pipe(
      implementRefactor,
      typeCheckFiles,
      fixTypeErrors,
      commitChanges,
      createPullRequest,
    ),
  ),
);
