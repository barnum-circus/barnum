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
 * Demonstrates: withResource, loop, forEach, constant, pipe, bindInput,
 * Option.collect, and postfix operators.
 *
 * Usage: pnpm exec tsx run.ts
 */

import path from "node:path";
import { fileURLToPath } from "node:url";

import {
  runPipeline,
  pipe,
  forEach,
  constant,
  withResource,
  Option,
} from "@barnum/barnum/pipeline";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
const srcDir = path.resolve(__dirname, "src");

import {
  listTargetFiles,
  analyze,
  assessWorthiness,
  implementAndReview,
  createBranchWorktree,
} from "./handlers/refactor";
import { deleteWorktree } from "./handlers/git";

console.error("=== Running identify-and-address-refactors workflow ===\n");

runPipeline(
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
        create: createBranchWorktree,
        action: implementAndReview,
        dispose: deleteWorktree,
      }),
    ),
  ),
);
