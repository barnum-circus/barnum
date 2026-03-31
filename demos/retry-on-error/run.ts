/**
 * Retry-on-error demo: fallible pipeline with tryCatch, withTimeout,
 * loop, and earlyReturn. Catches handler errors and timeouts in the same
 * catch block, but exits immediately on catastrophic failures via earlyReturn.
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  workflowBuilder,
  pipe,
  loop,
  earlyReturn,
  tryCatch,
  withTimeout,
} from "@barnum/barnum/src/ast.js";
import { constant, drop } from "@barnum/barnum/src/builtins.js";
import { stepA, stepB, stepC, logError } from "./handlers/steps.js";

console.error("=== Retry-on-error demo ===\n");

await workflowBuilder()
  .workflow(() =>
    earlyReturn((earlyReturn) =>
      loop<any, never>((recur, done) =>
        pipe(
          drop<any>(),
          tryCatch(
            (throwError) =>
              pipe(
                // stepA may fail — unwrapOr surfaces the error as a Result
                stepA.unwrapOr(throwError).drop(),

                // stepB may fail and may take unreasonably long
                withTimeout(constant(2_000), stepB.unwrapOr(throwError))
                  .mapErr(constant("stepB: timed out"))
                  .unwrapOr(throwError)
                  .drop(),

                // If stepC errors, it's catastrophic — exit immediately
                stepC.mapErr(drop()).unwrapOr(earlyReturn).drop(),
                done,
              ),

            // An error occurred — log it and retry the loop
            logError.drop().then(recur),
          ),
        ),
      ),
    ),
  )
  .run();
