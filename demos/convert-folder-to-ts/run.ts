/**
 * Convert-folder-to-TS demo: JS → TypeScript migration workflow.
 *
 * Pipeline:
 *   1. Setup — clean output directory
 *   2. List JS files in src/ (returns { file, outputPath }[])
 *   3. For each file:
 *      a. Extract "file", run migrate, .augment() merges { content } back
 *      b. writeFile receives { content, file, outputPath }
 *   4. Type-check/fix loop — run tsc, classify errors, fix or finish
 *
 * Demonstrates: pipe, forEach, loop, createHandlerWithConfig,
 * and postfix operators (.branch, .drop, .augment).
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

type FileEntry = { file: string; outputPath: string };

console.error("=== Running JS → TypeScript migration workflow ===\n");

await workflowBuilder()
  .workflow(() =>
    pipe(
      // Phase 1: Setup and discover files
      setup,
      listFiles,

      // Phase 2: For each file, migrate and write.
      // extractField("file") pulls the path string, migrate converts it,
      // .augment() merges { content } back into { file, outputPath }.
      forEach(
        pipe(
          pipe(
            extractField<FileEntry, "file">("file"),
            migrate({ to: "Typescript" }),
          ).augment().pick("content", "outputPath"),
          writeFile,
        ),
      ).drop(),

      // Phase 3: Type-check / fix loop
      loop(
        pipe(typeCheck, classifyErrors).branch({
          HasErrors: pipe(
            extractField<Extract<ClassifyResult, { kind: "HasErrors" }>, "errors">("errors"),
            forEach(fix).drop(),
            recur<any>(),
          ),
          Clean: done<any>(),
        }),
      ),
    ),
  )
  .run();
