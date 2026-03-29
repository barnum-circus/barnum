/**
 * Convert-folder-to-TS demo: JS → TypeScript migration workflow.
 *
 * Pipeline:
 *   1. Setup — clean output directory
 *   2. List JS files in src/ (returns { file, outputPath }[])
 *   3. For each file:
 *      a. parallel(migrate → content, identity → file metadata)
 *      b. merge into { content, file, outputPath }
 *      c. writeFile
 *   4. Type-check/fix loop — run tsc, classify errors, fix or finish
 *
 * Demonstrates: pipe, forEach, loop, branch, parallel, merge, drop,
 * extractField, recur, done, identity, createHandlerWithConfig.
 *
 * Usage: pnpm exec tsx convert-folder-to-ts.ts
 */

import {
  configBuilder,
  pipe,
  forEach,
  loop,
  branch,
  parallel,
} from "@barnum/barnum/src/ast.js";
import {
  drop,
  extractField,
  identity,
  merge,
  recur,
  done,
} from "@barnum/barnum/src/builtins.js";

import { setup, listFiles, migrate, writeFile } from "./handlers/convert.js";
import { typeCheck, classifyErrors, fix } from "./handlers/type-check-fix.js";

console.error("=== Running JS → TypeScript migration workflow ===\n");

await configBuilder()
  .workflow(() =>
    pipe(
      // Phase 1: Setup and discover files
      setup,
      listFiles,

      // Phase 2: For each file, migrate and write
      //
      // The merge trick: parallel runs migrate and identity on the same
      // input. migrate produces { content }, identity preserves { file,
      // outputPath }. merge() combines them into a single object for
      // writeFile.
      forEach(
        pipe(
          parallel(
            pipe(extractField("file"), migrate({ to: "Typescript" })),
            identity(),
          ),
          merge(),
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
