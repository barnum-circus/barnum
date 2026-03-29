/**
 * Convert-folder-to-TS demo: JS → TypeScript migration workflow.
 *
 * Pipeline:
 *   1. Setup — clean output directory
 *   2. List JS files in src/ (returns { file, outputPath }[])
 *   3. For each file:
 *      a. augment: run migrate on "file" field, merge { content } back
 *      b. writeFile receives { content, file, outputPath }
 *   4. Type-check/fix loop — run tsc, classify errors, fix or finish
 *
 * Demonstrates: pipe, forEach, loop, branch, augment, drop,
 * extractField, recur, done, createHandlerWithConfig.
 *
 * Usage: pnpm exec tsx convert-folder-to-ts.ts
 */

import {
  workflowBuilder,
  pipe,
  forEach,
  loop,
  branch,
} from "@barnum/barnum/src/ast.js";
import {
  augment,
  drop,
  extractField,
  recur,
  done,
} from "@barnum/barnum/src/builtins.js";

import { setup, listFiles, migrate, writeFile } from "./handlers/convert.js";
import { typeCheck, classifyErrors, fix } from "./handlers/type-check-fix.js";

console.error("=== Running JS → TypeScript migration workflow ===\n");

await workflowBuilder()
  .workflow(() =>
    pipe(
      // Phase 1: Setup and discover files
      setup,
      listFiles,

      // Phase 2: For each file, migrate and write
      //
      // augment runs migrate on the "file" field and merges { content }
      // back into the original { file, outputPath } object.
      forEach(
        pipe(
          augment(pipe(extractField("file"), migrate({ to: "Typescript" }))),
          writeFile,
        ),
      ),
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
