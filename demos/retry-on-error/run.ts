/**
 * Retry-on-error demo: fallible pipeline with tryCatch, invokeWithTimeout,
 * and loop. Catches both handler errors and timeouts in the same catch block.
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  workflowBuilder,
  pipe,
  loop,
  tryCatch,
  invokeWithTimeout,
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
          invokeWithTimeout(stepA, constant(10_000), throwError).drop(),
          invokeWithTimeout(stepB, constant(2_000), throwError).drop(),
          invokeWithTimeout(stepC, constant(10_000), throwError),
          done<never, string>(),
        ),
        logError.drop().then(recur<never, string>()),
      ),
    ),
  )
  .run();
