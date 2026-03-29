/**
 * Identify-and-address-refactors demo: automated codebase refactoring.
 *
 * Pipeline:
 *   1. List all files in target folder
 *   2. For each file, analyze for refactoring opportunities
 *   3. Flatten into a single refactor list
 *   4. For each refactor, within an RAII worktree:
 *      a. Implement the refactor
 *      b. Commit changes
 *      c. Type-check/fix cycle (mutual recursion via registerSteps)
 *      d. Judge/revise loop — review quality, apply feedback if needed
 *      e. Create PR
 *   5. Delete worktree (RAII dispose)
 *
 * Demonstrates: registerSteps, stepRef (mutual recursion), withResource
 * (RAII), loop, branch, forEach, flatten, constant, pipe, drop.
 *
 * Usage: pnpm exec tsx identify-and-address-refactors.ts
 */

import {
  configBuilder,
  pipe,
  forEach,
  loop,
  branch,
} from "@barnum/barnum/src/ast.js";
import {
  constant,
  drop,
  extractField,
  flatten,
  withResource,
  recur,
  done,
} from "@barnum/barnum/src/builtins.js";

import {
  listTargetFiles,
  analyze,
  createWorktree,
  deleteWorktree,
  implement,
  commit,
  judgeRefactor,
  classifyJudgment,
  applyFeedback,
  createPR,
} from "./handlers/refactor.js";
import { typeCheck, classifyErrors, fix } from "./handlers/type-check-fix.js";

console.error("=== Running identify-and-address-refactors workflow ===\n");

await configBuilder()
  // Mutual recursion: TypeCheckFix → branch → HasErrors → fix → TypeCheckFix
  .registerSteps(({ stepRef }) => ({
    TypeCheckFix: pipe(
      typeCheck,
      classifyErrors,
      branch({
        HasErrors: pipe(
          extractField("errors"),
          forEach(fix),
          drop(),
          stepRef("TypeCheckFix"),
        ),
        Clean: drop(),
      }),
    ),
  }))
  .workflow(({ steps }) =>
    pipe(
      constant({ folder: "/path/to/project" }),
      listTargetFiles,

      // Analyze each file for refactoring opportunities
      forEach(analyze),
      flatten(),

      // For each refactor: create worktree → work → create PR → cleanup
      forEach(
        withResource({
          create: createWorktree,
          action: pipe(
            implement,
            commit,
            drop(),

            // Type-check/fix cycle (registered step for mutual recursion)
            steps.TypeCheckFix,

            // Judge/revise loop: review the refactor, revise if needed.
            // drop() discards the TypeCheckFix output — judgeRefactor
            // reads the filesystem, not pipeline data.
            loop(
              pipe(
                drop(),
                judgeRefactor,
                classifyJudgment,
                branch({
                  NeedsWork: pipe(
                    extractField("instructions"),
                    applyFeedback,
                    drop(),
                    steps.TypeCheckFix,
                    recur(),
                  ),
                  Approved: done(),
                }),
              ),
            ),

            drop(),
            createPR,
          ),
          dispose: deleteWorktree,
        }),
      ),
    ),
  )
  .run();
