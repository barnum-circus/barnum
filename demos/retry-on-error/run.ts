/**
 * Retry-on-error demo: fallible pipeline with tryCatch, withTimeout,
 * and loop. Catches handler errors and timeouts in the same catch block
 * for retry. Catastrophic failures (stepA) exit the loop immediately
 * via done; successful completion falls through naturally.
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  runPipeline,
  pipe,
  loop,
  tryCatch,
  withTimeout,
  constant,
  drop,
} from "@barnum/barnum";
import type { TypedAction } from "@barnum/barnum";
import { stepA, stepB, stepC, logError } from "./handlers/steps.js";

console.error("=== Retry-on-error demo ===\n");

// throwError is a first-class value — you can pass it to helper functions
// that build sub-pipelines, keeping the main pipeline flat and readable.
function stepBWithTimeout(
  throwError: TypedAction<string, never>,
) {
  return withTimeout(constant(2_000), stepB.unwrapOr(throwError))
    .mapErr(constant("stepB: timed out"))
    .unwrapOr(throwError)
    .drop();
}

runPipeline(
  loop((recur, done) =>
    tryCatch(
      (throwError) =>
        pipe(
          // stepA may fail catastrophically — exit the loop immediately
          stepA.mapErr(drop).unwrapOr(done).drop(),

          // stepB may fail and may take unreasonably long
          stepBWithTimeout(throwError),

          // stepC may fail — retry via catch
          stepC.unwrapOr(throwError).drop(),
        ),

      // An error occurred — log it and retry the loop
      logError.then(recur),
    ),
  ),
);
