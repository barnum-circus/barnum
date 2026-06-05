/**
 * Simple workflow demo: list files, refactor, type-check, fix, commit, and PR.
 *
 * Usage: pnpm exec tsx run.ts
 */

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

listFiles.iterate().map(pr).collect().run();
