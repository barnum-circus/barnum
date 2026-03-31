/**
 * Retry-on-error demo: fallible pipeline with tryCatch, withTimeout,
 * and loop. Catches both handler errors and timeouts in the same catch block.
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  workflowBuilder,
  pipe,
  loop,
  tryCatch,
  withTimeout,
} from "@barnum/barnum/src/ast.js";
import {
  constant,
  recur,
  done,
} from "@barnum/barnum/src/builtins.js";
import { stepA, stepB, stepC, logError } from "./handlers/steps.js";

console.error("=== Retry-on-error demo ===\n");

await workflowBuilder()
  .workflow(() =>
    loop(
      tryCatch(
        (throwError) => pipe(
          stepA.unwrapOr(throwError).drop(),
          withTimeout(constant(2_000), stepB.unwrapOr(throwError))
            .mapErr(constant("stepB: timed out"))
            .unwrapOr(throwError)
            .drop(),
          stepC.unwrapOr(throwError),
          done<never, string>(),
        ),
        logError.drop().then(recur<never, string>()),
      ),
    ),
  )
  .run();
