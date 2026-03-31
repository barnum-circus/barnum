/**
 * Convert-folder-to-TS demo: JS → TypeScript migration workflow.
 *
 * Pipeline:
 *   1. Clean the output directory
 *   2. List JS files in src/
 *   3. For each file: migrate to TS and write the output
 *   4. Type-check/fix loop until clean
 *
 * Demonstrates: pipe, forEach, loop, bindInput, all,
 * createHandlerWithConfig, and postfix operators (.get, .pick, .merge, .drop, .then).
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

import { setup, listFiles, migrate, writeFile } from "./handlers/convert.js";
import { typeCheck, classifyErrors, fix } from "./handlers/type-check-fix.js";

type FileEntry = { file: string; outputPath: string };

console.error("=== Running JS → TypeScript migration workflow ===\n");

await workflowBuilder()
  .workflow(() =>
    pipe(
      setup,
      listFiles,

      // For each file: extract the file path, migrate to TS, then combine
      // the migrated content with the original output path for writing.
      forEach(
        bindInput<FileEntry>((entry) =>
          pipe(
            entry.get("file"),
            migrate({ to: "Typescript" }),
            bindInput<{ content: string }>((migrateResult) =>
              all(migrateResult, entry.pick("outputPath")).merge().then(writeFile),
            ),
          ),
        ),
      ).drop(),
    ).then(
      // Type-check/fix loop: run tsc, fix any errors, repeat until clean.
      loop<void>((recur, done) =>
        pipe(typeCheck, classifyErrors).branch({
          HasErrors: pipe(forEach(fix).drop(), recur),
          Clean: done,
        }),
      ),
    ),
  )
  .run();
