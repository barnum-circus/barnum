/**
 * Convert-folder-to-TS demo: JS → TypeScript migration workflow.
 *
 * Pipeline:
 *   1. Setup — clean output directory
 *   2. List JS files in src/ (returns { file, outputPath }[])
 *   3. For each file:
 *      a. bindInput captures the FileEntry, extracts "file", runs migrate
 *      b. all(identity, pick("outputPath")) + merge combines { content, outputPath }
 *      c. writeFile receives { content, outputPath }
 *   4. Type-check/fix loop — run tsc, classify errors, fix or finish
 *
 * Demonstrates: pipe, forEach, loop, bindInput, all, merge,
 * createHandlerWithConfig, and postfix operators (.branch, .drop).
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  workflowBuilder,
  pipe,
  forEach,
  loop,
  all,
  bindInput,
} from "@barnum/barnum/src/ast.js";
import {
  extractField,
  identity,
  merge,
  pick,
  recur,
  done,
} from "@barnum/barnum/src/builtins.js";

import { setup, listFiles, migrate, writeFile } from "./handlers/convert.js";
import { typeCheck, classifyErrors, fix } from "./handlers/type-check-fix.js";

type FileEntry = { file: string; outputPath: string };

console.error("=== Running JS → TypeScript migration workflow ===\n");

await workflowBuilder()
  .workflow(() =>
    pipe(
      // Phase 1: Setup and discover files
      setup,
      listFiles,

      // Phase 2: For each file, migrate and write.
      // bindInput captures the FileEntry as a VarRef. The pipeline
      // extracts "file", runs migrate to get { content }, then
      // combines it with { outputPath } from the original entry.
      forEach(
        bindInput<FileEntry>((entry) => pipe(
          pipe(entry, extractField("file"), migrate({ to: "Typescript" })),
          all(identity(), pipe(entry, pick("outputPath"))),
          merge(),
          writeFile,
        )),
      ).drop(),

      // Phase 3: Type-check / fix loop
      loop(
        pipe(typeCheck, classifyErrors).branch({
          HasErrors: pipe(
            forEach(fix).drop(),
            recur<any, any>(),
          ),
          Clean: done<any, any>(),
        }),
      ),
    ),
  )
  .run();
