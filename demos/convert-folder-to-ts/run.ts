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

import { runPipeline, pipe } from "@barnum/barnum";
import { setup, listFiles, migrate } from "./handlers/convert.js";
import { typeCheckFix } from "./handlers/type-check-fix.js";

console.error("=== Running JS → TypeScript migration workflow ===\n");

runPipeline(
  pipe(
    setup,
    listFiles
      .forEach(migrate({ to: "Typescript" }))
      .drop(),
    typeCheckFix,
  ),
);
