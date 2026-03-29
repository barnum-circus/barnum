/**
 * Identify-and-address-refactors demo: automated codebase refactoring.
 *
 * Pipeline:
 *   1. List all files in target folder
 *   2. For each file, analyze for refactoring opportunities
 *   3. Flatten into a single refactor list
 *   4. For each refactor, within an RAII worktree:
 *      a. merge() the [resource, input] tuple into a flat context
 *      b. Implement the refactor (tap: side effect, preserves context)
 *      c. Commit changes (tap: side effect, preserves context)
 *      d. Type-check/fix cycle (tap: side effect, preserves context)
 *      e. Judge/revise loop (tap: side effect, preserves context)
 *      f. Create PR (augment: enriches context with prUrl)
 *   5. Delete worktree (RAII dispose — receives the resource directly)
 *
 * Demonstrates: registerSteps, stepRef (mutual recursion), withResource
 * (RAII with tuple), loop, forEach, constant, pipe, merge, augment, tap,
 * and postfix operators (.branch, .flatten, .drop).
 *
 * Usage: pnpm exec tsx identify-and-address-refactors.ts
 */

import {
  workflowBuilder,
  pipe,
  forEach,
  loop,
} from "@barnum/barnum/src/ast.js";
import {
  augment,
  constant,
  drop,
  extractField,
  merge,
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

await workflowBuilder()
  .registerSteps(({ stepRef }) => ({
    // Mutual recursion: TypeCheck → Fix → TypeCheck
    //
    // TypeCheck runs tsc, classifies the result, and either dispatches to
    // Fix (HasErrors) or exits (Clean). Fix applies fixes and jumps back
    // to TypeCheck. Neither step can be defined without referencing the
    // other, so both must be registered in the same batch via stepRef.
    TypeCheck: pipe(typeCheck, classifyErrors).branch({
      HasErrors: pipe(extractField("errors"), stepRef("Fix")),
      Clean: drop(),
    }),
    Fix: forEach(fix).drop().then(stepRef("TypeCheck")),

    // The action that runs inside each worktree.
    //
    // withResource passes [resource, input] as a tuple. merge() flattens
    // it into a single context object so downstream handlers can access
    // both resource fields (worktreePath, branch) and input fields
    // (file, description, scope).
    //
    // Side-effectful steps use tap() to preserve this context through
    // operations that don't produce meaningful output.
    ImplementAndReview: pipe(
      merge(),

      // Side effects: implement refactor and commit changes
      tap(implement),
      tap(commit),

      // Type-check/fix cycle (mutual recursion: TypeCheck ↔ Fix)
      tap(stepRef("TypeCheck")),

      // Judge/revise loop: review the refactor, revise if needed.
      // drop() discards the tap context — judgeRefactor takes no input.
      tap(
        loop(
          pipe(drop(), judgeRefactor, classifyJudgment).branch({
            NeedsWork: pipe(
              extractField("instructions"),
              applyFeedback.drop().then(stepRef("TypeCheck")).then(recur()),
            ),
            Approved: done(),
          }),
        ),
      ),

      // Create PR: generic handler, augment merges { prUrl } back.
      augment(pipe(preparePRInput, createPR)),
    ),
  }))
  .workflow(({ steps }) =>
    pipe(
      constant({ folder: "/path/to/project" }),
      listTargetFiles,

      // Analyze each file for refactoring opportunities
      forEach(analyze).flatten(),

      // For each refactor: create worktree → work → create PR → cleanup
      //
      // withResource passes [resource, refactor] to the action as a tuple.
      // dispose receives the resource directly (just { worktreePath, branch }).
      forEach(
        withResource({
          create: pipe(deriveBranch, createWorktree),
          action: steps.ImplementAndReview,
          dispose: deleteWorktree,
        }),
      ),
    ),
  )
  .run();
