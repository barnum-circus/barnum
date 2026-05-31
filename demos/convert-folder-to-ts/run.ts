/**
 * Convert-folder-to-TS demo: JS → TypeScript migration workflow.
 *
 * Pipeline:
 *   1. bindInput captures the project config (inputDir, outputDir)
 *   2. Clean the output directory (side-effect)
 *   3. List JS files and migrate each to TS
 *   4. Type-check/fix loop until clean
 *
 * Demonstrates: pipe, Iterator, loop, bindInput,
 * createHandlerWithConfig, and postfix operators (.drop, .then, .iterate, .map, .collect).
 *
 * Usage: pnpm exec tsx run.ts
 */

import { runPipeline, bindInput } from "@barnum/barnum/pipeline";
import type { ProjectConfig } from "./handlers/convert";
import { setup, listFiles, migrate } from "./handlers/convert";
import { typeCheckFix } from "./handlers/type-check-fix";
import { baseDir } from "./handlers/lib";
import path from "node:path";

console.error("=== Running JS → TypeScript migration workflow ===\n");

runPipeline(
  bindInput<ProjectConfig, null>((config) => {
    const setupResult = setup.call(config);
    const migrated = listFiles
      .call(config)
      .iterate()
      .map(migrate({ to: "Typescript" }))
      .collect()
      .drop()
      .call(setupResult);
    return typeCheckFix.call(migrated);
  }),
  {
    inputDir: path.join(baseDir, "src"),
    outputDir: path.join(baseDir, "out"),
  },
);
