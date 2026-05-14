/**
 * Analyze-file demo: run three independent analyses on a single file,
 * each wrapped with retry logic.
 *
 * Demonstrates: withRetry as a higher-order pipeline function that takes
 * a fallible handler and returns a pipeline with retry behavior.
 *
 * Usage: pnpm exec tsx run.ts
 */

import type { Pipeable, TypedAction, Result } from "@barnum/barnum/pipeline";
import {
  runPipeline,
  all,
  loop,
  drop,
  bindInput,
  earlyReturn,
  tryCatch,
} from "@barnum/barnum/pipeline";
import {
  analyzeClassComponents,
  analyzeImpossibleStates,
  analyzeErrorHandling,
} from "./handlers/analyze";

/**
 * Wrap a fallible handler with infinite retry. The handler must return
 * Result<TOut, TErr>. On Ok, the value passes through. On Err,
 * the handler is retried with the original input.
 *
 * This is a plain function that returns a TypedAction — pipeline
 * composition is just function composition. The handler is taken
 * as a parameter and composed into a loop.
 */
function withRetry<TIn, TOut>(
  action: Pipeable<TIn, Result<TOut, string>>,
): TypedAction<TIn, TOut> {
  return bindInput<TIn, TOut>((originalInput) =>
    earlyReturn<TOut>((ret) =>
      loop<void, void>((recur, _done) =>
        tryCatch(
          (throwError) =>
            originalInput.then(action).unwrapOr(throwError).then(ret),
          drop.then(recur),
        ),
      ),
    ),
  );
}

console.error("=== Analyze-file with retry demo ===\n");

runPipeline(
  all(
    withRetry(analyzeClassComponents),
    withRetry(analyzeImpossibleStates),
    withRetry(analyzeErrorHandling),
  ),
  "source/index.ts",
);
