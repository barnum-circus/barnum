/**
 * Identify-and-address-refactors demo: automated codebase refactoring.
 *
 * Pipeline:
 *   1. List all files in the target folder
 *   2. For each file, analyze for refactoring opportunities
 *   3. Flatten into a single refactor list
 *   4. Filter out refactors that aren't worth doing
 *   5. For each worthwhile refactor, within a git worktree:
 *      a. Implement the refactor
 *      b. Type-check/fix cycle until clean
 *      c. Judge quality, revise if needed
 *      d. Commit and create a PR
 *   6. Clean up the worktree
 *
 * Demonstrates: registerSteps, withResource, loop, forEach, constant,
 * pipe, bindInput, Option.collect, and postfix operators.
 *
 * Usage: pnpm exec tsx run.ts
 */

import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  workflowBuilder,
  pipe,
  forEach,
  loop,
  bindInput,
} from "@barnum/barnum/src/ast.js";
import {
  constant,
  drop,
  pick,
  withResource,
  Option,
} from "@barnum/barnum/src/builtins.js";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const srcDir = path.resolve(__dirname, "src");

import {
  listTargetFiles,
  analyze,
  assessWorthiness,
  deriveBranch,
  preparePRInput,
  implement,
  commit,
  judgeRefactor,
  classifyJudgment,
  applyFeedback,
  type Refactor,
} from "./handlers/refactor.js";
import { createWorktree, deleteWorktree, createPR } from "./handlers/git.js";
import { typeCheck, classifyErrors, fix } from "./handlers/type-check-fix.js";

console.error("=== Running identify-and-address-refactors workflow ===\n");

// withResource merges the resource (worktree) into the input (Refactor),
// giving ImplementAndReview all five fields.
type ImplementAndReviewParams = Refactor & { worktreePath: string; branch: string };

await workflowBuilder()
  // Type-check/fix: run tsc, fix errors, repeat until clean.
  .registerSteps({
    TypeCheckFix: loop((recur) =>
      pipe(typeCheck, classifyErrors).branch({
        HasErrors: pipe(forEach(fix).drop(), recur),
        Clean: drop,
      }),
    ),
  })
  // Implement a refactor, get it passing, and open a PR.
  .registerSteps(({ steps }) => ({
    ImplementAndReview: bindInput<ImplementAndReviewParams>((implementAndReviewParams) => pipe(
      implementAndReviewParams.pick("worktreePath", "description").then(implement).drop(),
      implementAndReviewParams.pick("worktreePath").then(steps.TypeCheckFix).drop(),

      // Judge quality; revise and re-check if needed.
      loop((recur) =>
        pipe(judgeRefactor, classifyJudgment).branch({
          NeedsWork: pipe(applyFeedback, steps.TypeCheckFix).drop().then(recur),
          Approved: drop,
        }),
      ).drop(),

      // Commit and open a PR only after all fixes and revisions are done.
      implementAndReviewParams.pick("worktreePath").then(commit).drop(),
      pipe(implementAndReviewParams.pick("branch", "description"), preparePRInput, createPR),
    )),
  }))
  .workflow(({ steps }) =>
    pipe(
      constant({ folder: srcDir }),
      listTargetFiles,

      // Analyze each file for refactoring opportunities.
      forEach(analyze).flatten(),

      // Keep only worthwhile refactors (Option.collect filters out Nones).
      forEach(assessWorthiness).then(Option.collect()),

      // For each refactor: create a worktree, do the work, open a PR, clean up.
      forEach(
        withResource({
          create: pipe(pick<Refactor, ["description"]>("description"), deriveBranch, createWorktree),
          action: steps.ImplementAndReview,
          dispose: deleteWorktree,
        }),
      ),
    ),
  )
  .run();
