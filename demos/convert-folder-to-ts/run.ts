/**
 * Convert-folder-to-TS demo: JS → TypeScript migration workflow.
 *
 * Pipeline:
 *   1. Setup — clean output directory
 *   2. List JS files in src/ (returns { file, outputPath }[])
 *   3. For each file: migrate → writeFile
 *   4. Type-check/fix loop — run tsc, classify errors, fix or finish
 *
 * Demonstrates: pipe, forEach, loop, createHandlerWithConfig,
 * and postfix operators (.branch, .drop).
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  workflowBuilder,
  pipe,
  forEach,
  loop,
} from "@barnum/barnum/src/ast.js";
import {
  extractField,
  recur,
  done,
} from "@barnum/barnum/src/builtins.js";

import { setup, listFiles, migrate, writeFile } from "./handlers/convert.js";
import { typeCheck, classifyErrors, fix, type ClassifyResult } from "./handlers/type-check-fix.js";

console.error("=== Running JS → TypeScript migration workflow ===\n");

await workflowBuilder()
  .workflow(() =>
    pipe(
      // Phase 1: Setup and discover files
      setup,
      listFiles,

      // Phase 2: For each file, migrate and write.
      // migrate accepts { file, outputPath } and returns { content, outputPath }.
      forEach(pipe(migrate({ to: "Typescript" }), writeFile)).drop(),

      // Phase 3: Type-check / fix loop
      //
      // typeCheck operates on the filesystem, not a pipeline value.
      // recur() feeds the fix results back, so .drop() discards them
      // before re-entering typeCheck.
      loop(
        pipe(typeCheck, classifyErrors).branch({
          HasErrors: pipe(
            extractField<Extract<ClassifyResult, { kind: "HasErrors" }>, "errors">("errors"),
            forEach(fix).drop(),
            recur(),
          ),
          Clean: done(),
        }),
      ),
    ),
  )
  .run();
