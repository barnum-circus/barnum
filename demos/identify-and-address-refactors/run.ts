/**
 * Identify-and-address-refactors demo: automated codebase refactoring.
 *
 * Pipeline:
 *   1. List all files in target folder
 *   2. For each file, analyze for refactoring opportunities
 *   3. Flatten into a single refactor list
 *   4. Assess each refactor's worthiness (Option.collect filters out Nones)
 *   5. For each worthwhile refactor, within an RAII worktree:
 *      a. Implement the refactor (env VarRef provides context, .drop() discards output)
 *      b. Commit changes (env VarRef provides context, .drop() discards output)
 *      c. Type-check/fix cycle (env VarRef provides context, .drop() discards output)
 *      d. Judge/revise loop (.drop() discards output)
 *      e. Create PR (env VarRef provides branch + description)
 *   6. Delete worktree (RAII dispose — receives the resource directly)
 *
 * Demonstrates: registerSteps, stepRef (mutual recursion), withResource
 * (RAII), loop, forEach, constant, pipe, bindInput, Option.collect,
 * and postfix operators (.branch, .flatten, .drop, .then).
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
  recur,
  done,
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

// The full context type inside the worktree action. withResource merges
// the resource (createWorktree output) with the input (Refactor), giving
// all five fields.
type ImplementAndReviewParams = Refactor & { worktreePath: string; branch: string };

await workflowBuilder()
  .registerSteps(({ stepRef }) => ({
    // Mutual recursion: TypeCheck → Fix → TypeCheck
    //
    // TypeCheck runs tsc, classifies the result, and either dispatches to
    // Fix (HasErrors) or exits (Clean). Fix applies fixes and jumps back
    // to TypeCheck. Neither step can be defined without referencing the
    // other, so both must be registered in the same batch via stepRef.
    TypeCheck: pipe(typeCheck, classifyErrors).branch({
      HasErrors: stepRef("Fix"),
      Clean: drop(),
    }),
    Fix: pipe(forEach(fix).drop(), stepRef("TypeCheck")),

    // The action that runs inside each worktree.
    //
    // bindInput captures the full ImplementAndReviewParams as a VarRef (env). Each side
    // effect accesses the fields it needs through env, then .drop()
    // discards its output. No tap/augment needed — the VarRef provides
    // context independently of the pipeline flow.
    //
    // pick() still narrows to exactly the fields each handler expects —
    // invariance prevents passing extra fields across serialization boundaries.
    ImplementAndReview: bindInput<ImplementAndReviewParams>((env) => pipe(
      // Side effects: implement refactor and commit changes.
      pipe(env, pick<ImplementAndReviewParams, ["worktreePath", "description"]>("worktreePath", "description"), implement).drop(),
      pipe(env, pick<ImplementAndReviewParams, ["worktreePath"]>("worktreePath"), commit).drop(),

      // Type-check/fix cycle (mutual recursion: TypeCheck ↔ Fix)
      pipe(env, pick<ImplementAndReviewParams, ["worktreePath"]>("worktreePath"), stepRef("TypeCheck")).drop(),

      // Judge/revise loop: review the refactor, revise if needed.
      // drop() discards the pipeline value — judgeRefactor takes no input.
      loop(
        pipe(drop(), judgeRefactor, classifyJudgment).branch({
          NeedsWork: pipe(
            applyFeedback.drop(), stepRef("TypeCheck"), recur(),
          ),
          Approved: done(),
        }),
      ).drop(),

      // Create PR — env VarRef provides context independently.
      pipe(env, pick<ImplementAndReviewParams, ["branch", "description"]>("branch", "description"), preparePRInput, createPR),
    )),
  }))
  .workflow(({ steps }) =>
    pipe(
      constant({ folder: srcDir }),
      listTargetFiles,

      // Analyze each file for refactoring opportunities
      forEach(analyze).flatten(),

      // Filter: assess each refactor's worthiness.
      // assessWorthiness returns Option<Refactor> — Some if worth doing, None if not.
      // Option.collect() drops Nones and unwraps Somes into a flat Refactor[].
      forEach(assessWorthiness).then(Option.collect()),

      // For each refactor: create worktree → work → create PR → cleanup
      //
      // withResource merges the resource into each refactor, so the action
      // receives { worktreePath, branch, file, description, scope }.
      // pick() narrows Refactor to just {description} for deriveBranch —
      // invariance requires exact type matches at handler boundaries.
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
