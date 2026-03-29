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
  .registerSteps(({ stepRef }) => ({
    // Mutual recursion: TypeCheck → Fix → TypeCheck
    //
    // TypeCheck runs tsc, classifies the result, and either dispatches to
    // Fix (HasErrors) or exits (Clean). Fix applies fixes and jumps back
    // to TypeCheck. Neither step can be defined without referencing the
    // other, so both must be registered in the same batch via stepRef.
    TypeCheck: pipe(
      typeCheck,
      classifyErrors,
      branch({
        HasErrors: pipe(extractField("errors"), stepRef("Fix")),
        Clean: drop(),
      }),
    ),
    Fix: pipe(forEach(fix), drop(), stepRef("TypeCheck")),

    // The action that runs inside each worktree. Implements the refactor,
    // commits, runs the type-check/fix cycle, judges quality in a loop,
    // and creates a PR.
    ImplementAndReview: pipe(
      implement,
      commit,
      drop(),

      // Type-check/fix cycle (mutual recursion: TypeCheck ↔ Fix)
      stepRef("TypeCheck"),

      // Judge/revise loop: review the refactor, revise if needed.
      // drop() discards the TypeCheck output — judgeRefactor
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
              stepRef("TypeCheck"),
              recur(),
            ),
            Approved: done(),
          }),
        ),
      ),

      drop(),
      createPR,
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
          action: steps.ImplementAndReview,
          dispose: deleteWorktree,
        }),
      ),
    ),
  )
  .run();
