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
import { stepA, stepB, stepC, stepD, logError } from "./handlers/steps.js";

console.error("=== Retry-on-error demo ===\n");

await workflowBuilder()
  .workflow(() =>
    earlyReturn<any, string, any>((earlyReturn) =>
      loop<any, any>((recur, done) =>
        pipe(
          drop<any>(),
          tryCatch(
            (throwError) => pipe(
              stepA.unwrapOr(throwError).drop(),
              withTimeout(constant(2_000), stepB.unwrapOr(throwError))
                .mapErr(constant("stepB: timed out"))
                .unwrapOr(throwError)
                .drop(),
              stepC.unwrapOr(throwError).drop(),
              stepD.unwrapOr(earlyReturn),
              done,
            ),
            logError.drop().then(recur),
          ),
        ),
      ),
    ),
  )
  .run();
