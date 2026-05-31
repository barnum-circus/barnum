/**
 * Simple workflow demo: list files, refactor, type-check, fix, commit, and PR.
 *
 * Usage: pnpm exec tsx run.ts
 */

import { runPipeline } from "@barnum/barnum/pipeline";
import {
  listFiles,
  implementRefactor,
  typeCheckFiles,
  fixTypeErrors,
  commitChanges,
  createPullRequest,
} from "./handlers/steps";

const typeChecked = typeCheckFiles.call(implementRefactor);
const fixed = fixTypeErrors.call(typeChecked);
const committed = commitChanges.call(fixed);
const pr = createPullRequest.call(committed);

runPipeline(listFiles.iterate().map(pr).collect());
