/**
 * Kitchen sink demo: JS → TypeScript migration workflow.
 *
 * Mirrors the barnum-demo pattern:
 *   1. Setup — clean output directory
 *   2. List JS files in src/
 *   3. For each file, migrate JS → TS (would invoke Claude in production)
 *   4. Type-check/fix loop — run tsc, classify errors, fix or finish
 *
 * Demonstrates: pipe, forEach, loop, branch, drop, extractField,
 * recur, done — all the core barnum combinators in a single workflow.
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  configBuilder,
  pipe,
  forEach,
  loop,
  branch,
} from "@barnum/barnum/src/ast.js";
import {
  drop,
  extractField,
  recur,
  done,
} from "@barnum/barnum/src/builtins.js";

import setup from "./handlers/setup.js";
import listFiles from "./handlers/list-files.js";
import migrate from "./handlers/migrate.js";
import typeCheck from "./handlers/type-check.js";
import classifyErrors from "./handlers/classify-errors.js";
import fix from "./handlers/fix.js";

console.error("=== Running JS → TypeScript migration workflow ===\n");

await configBuilder()
  .workflow(() =>
    pipe(
      // Phase 1: Setup and discover files
      setup,
      listFiles,

      // Phase 2: Migrate each JS file to TypeScript
      forEach(migrate),
      drop(),

      // Phase 3: Type-check / fix loop
      //
      // typeCheck operates on the filesystem, not a pipeline value.
      // recur() feeds the fix results back, so drop() discards them
      // before re-entering typeCheck.
      loop(
        pipe(
          typeCheck,
          classifyErrors,
          branch({
            HasErrors: pipe(extractField("errors"), forEach(fix), drop(), recur()),
            Clean: done(),
          }),
        ),
      ),
    ),
  )
  .run();
