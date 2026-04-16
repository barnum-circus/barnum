/**
 * Analyze-file demo: run three independent analyses on a single file.
 *
 * Demonstrates: runPipeline with input, all (parallel execution).
 *
 * Usage: pnpm exec tsx run.ts
 */

import { runPipeline, all } from "@barnum/barnum/pipeline";
import {
  analyzeClassComponents,
  analyzeImpossibleStates,
  analyzeErrorHandling,
} from "./handlers/analyze";

runPipeline(
  all(analyzeClassComponents, analyzeImpossibleStates, analyzeErrorHandling),
  "source/index.ts",
);
