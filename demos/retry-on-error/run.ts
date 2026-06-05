/**
 * Retry-on-error demo: fallible pipeline with tryCatch, withTimeout,
 * and loop. Catches handler errors and timeouts in the same catch block
 * for retry. Catastrophic failures (stepA) exit the loop immediately
 * via done; successful completion falls through naturally.
 *
 * Usage: pnpm exec tsx run.ts
 */

import { loop, tryCatch, withTimeout, constant } from "@barnum/barnum/pipeline";
import type { TypedAction } from "@barnum/barnum/pipeline";
import { stepA, stepB, stepC, logError } from "./handlers/steps";

console.error("=== Retry-on-error demo ===\n");

loop<void, string>((recur, done) =>
  tryCatch(
    (throwError) => {
      const afterA = stepA.unwrapOr(done).drop();
      const afterB = stepBWithTimeout(throwError).call(afterA);
      const result = stepC.unwrapOr(throwError).call(afterB);
      return done.call(result);
    },

    // An error occurred — log it and retry the loop
    recur.call(logError.drop()),
  ),
).run();

// throwError is a first-class value — you can pass it to helper functions
// that build sub-pipelines, keeping the main pipeline flat and readable.
function stepBWithTimeout(
  throwError: TypedAction<string, never>,
): TypedAction<void, null> {
  return withTimeout(constant(2_000), stepB.unwrapOr(throwError))
    .mapErr(constant("stepB: timed out"))
    .unwrapOr(throwError)
    .drop();
}
