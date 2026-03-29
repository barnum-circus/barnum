/**
 * Identify-and-address-refactors demo: automated codebase refactoring.
 *
 * Pipeline:
 *   1. List all files in target folder
 *   2. For each file, analyze for refactoring opportunities
 *   3. Flatten into a single refactor list
 *   4. Assess each refactor's worthiness (Option.collect filters out Nones)
 *   5. For each worthwhile refactor, within an RAII worktree:
 *      a. Implement the refactor (tap: side effect, preserves context)
 *      b. Commit changes (tap: side effect, preserves context)
 *      c. Type-check/fix cycle (tap: side effect, preserves context)
 *      d. Judge/revise loop (tap: side effect, preserves context)
 *      e. Create PR (augment: enriches context with prUrl)
 *   6. Delete worktree (RAII dispose — receives the resource directly)
 *
 * Demonstrates: registerSteps, stepRef (mutual recursion), withResource
 * (RAII), loop, forEach, constant, pipe, augment, tap, Option.collect,
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
} from "@barnum/barnum/src/ast.js";
import {
  augment,
  constant,
  drop,
  pick,
  tap,
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
// all five fields. Explicit type annotations on tap<Ctx>() are needed
// because tap infers TInput from its action argument, which may only
// declare a subset of the context fields. Without annotation, each tap
// narrows the pipe's flow type to its handler's input type.
type Ctx = Refactor & { worktreePath: string; branch: string };

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
    // withResource merges the resource with the original input into a
    // flat context object so downstream handlers can access both resource
    // fields (worktreePath, branch) and input fields (file, description,
    // scope).
    //
    // Side-effectful steps use tap() to preserve this context through
    // operations that don't produce meaningful output.
    ImplementAndReview: pipe(
      // Side effects: implement refactor and commit changes.
      // pick() narrows to exactly the fields each handler expects —
      // invariance prevents passing extra fields across serialization boundaries.
      tap(pipe(pick<Ctx, ["worktreePath", "description"]>("worktreePath", "description"), implement)),
      tap(pipe(pick<Ctx, ["worktreePath"]>("worktreePath"), commit)),

      // Type-check/fix cycle (mutual recursion: TypeCheck ↔ Fix)
      tap<Ctx, any, "TypeCheck">(stepRef("TypeCheck")),

      // Judge/revise loop: review the refactor, revise if needed.
      // drop<any>() discards the tap context — judgeRefactor takes no input.
      tap<Ctx, any, "TypeCheck">(
        loop(
          pipe(drop<any>(), judgeRefactor, classifyJudgment).branch({
            NeedsWork: pipe(
              applyFeedback.drop(), stepRef("TypeCheck"), recur<any, any>(),
            ),
            Approved: done<any, any>(),
          }),
        ),
      ),

      // Create PR: pick the fields preparePRInput needs, augment merges { prUrl } back.
      augment<Ctx, { prUrl: string }>(pipe(
        pick<Ctx, ["branch", "description"]>("branch", "description"),
        preparePRInput,
        createPR,
      )),
    ),
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
      forEach(assessWorthiness).then(Option.collect<Refactor>()),

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
