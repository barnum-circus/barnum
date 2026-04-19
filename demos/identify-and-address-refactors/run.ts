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
 * Demonstrates: withResource, loop, Iterator, constant, bindInput,
 * and postfix operators (.then, .iterate, .flatMap, .map, .collect).
 *
 * Usage: pnpm exec tsx run.ts
 */

import path from "node:path";
import { fileURLToPath } from "node:url";

import { runPipeline, constant, withResource } from "@barnum/barnum/pipeline";

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
  constant({ folder: srcDir })
    .then(listTargetFiles)
    .iterate()
    .flatMap(analyze)
    .flatMap(assessWorthiness)
    .map(
      withResource({
        create: createBranchWorktree,
        action: implementAndReview,
        dispose: deleteWorktree,
      }),
    )
    .collect(),
);
