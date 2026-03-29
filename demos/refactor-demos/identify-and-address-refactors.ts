/**
 * Identify-and-address-refactors demo: automated codebase refactoring.
 *
 * Pipeline:
 *   1. List all files in target folder
 *   2. For each file, analyze for refactoring opportunities
 *   3. Flatten into a single refactor list
 *   4. For each refactor, within an RAII worktree:
 *      a. Implement the refactor (tap: side effect, preserves context)
 *      b. Commit changes (tap: side effect, preserves context)
 *      c. Type-check/fix cycle (tap: side effect, preserves context)
 *      d. Judge/revise loop (tap: side effect, preserves context)
 *      e. Create PR (augment: enriches context with prUrl)
 *   5. Delete worktree (RAII dispose)
 *
 * Demonstrates: registerSteps, stepRef (mutual recursion), withResource
 * (RAII), loop, branch, forEach, flatten, constant, pipe, drop,
 * augment, tap.
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
  augment,
  constant,
  drop,
  extractField,
  flatten,
  tap,
  withResource,
  recur,
  done,
} from "@barnum/barnum/src/builtins.js";

import {
  listTargetFiles,
  analyze,
  deriveBranch,
  preparePRInput,
  implement,
  commit,
  judgeRefactor,
  classifyJudgment,
  applyFeedback,
} from "./handlers/refactor.js";
import { createWorktree, deleteWorktree, createPR } from "./handlers/git.js";
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

    // The action that runs inside each worktree. Side-effectful steps
    // use tap() to preserve the pipeline context ({ ...Refactor,
    // worktreePath, branch }) through operations that don't produce
    // meaningful output.
    ImplementAndReview: pipe(
      // Side effects: implement refactor and commit changes
      tap(implement),
      tap(commit),

      // Type-check/fix cycle (mutual recursion: TypeCheck ↔ Fix)
      tap(stepRef("TypeCheck")),

      // Judge/revise loop: review the refactor, revise if needed.
      tap(
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
      ),

      // Create PR: generic handler, augment merges { prUrl } back into
      // context. The context still has worktreePath (needed by dispose).
      augment(pipe(preparePRInput, createPR)),
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
      //
      // create uses augment to merge worktree fields ({ worktreePath,
      // branch }) back into the Refactor object, giving the action the
      // full context it needs.
      forEach(
        withResource({
          create: augment(pipe(deriveBranch, createWorktree)),
          action: steps.ImplementAndReview,
          dispose: deleteWorktree,
        }),
      ),
    ),
  )
  .run();
