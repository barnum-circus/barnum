/**
 * Simple workflow demo: list files, refactor, type-check, fix, commit, and PR.
 *
 * Usage: pnpm exec tsx run.ts
 */

import { workflowBuilder, pipe } from "@barnum/barnum/src/ast.js";
import {
  listFiles,
  implementRefactor,
  typeCheckFiles,
  fixTypeErrors,
  commitChanges,
  createPullRequest,
} from "./handlers/steps.js";

await workflowBuilder()
  .workflow(() =>
    listFiles
      .forEach(
        pipe(
          implementRefactor,
          typeCheckFiles,
          fixTypeErrors,
          commitChanges,
          createPullRequest,
        ),
      )
      .drop(),
  )
  .run();
