/**
 * Retry-on-error demo: fallible pipeline with tryCatch + invokeWithThrow.
 *
 * Pipeline:
 *   loop(
 *     tryCatch(
 *       (throwError) => pipe(
 *         invokeWithThrow(stepA, throwError),
 *         drop(), invokeWithThrow(stepB, throwError),
 *         drop(), invokeWithThrow(stepC, throwError),
 *         done(),
 *       ),
 *       pipe(logError, recur()),
 *     ),
 *   )
 *
 * Each step randomly succeeds or fails. On any error, the catch handler
 * logs it and recurs. On success through all three steps, the loop breaks.
 *
 * Demonstrates: tryCatch, invokeWithThrow, loop, pipe, drop, done, recur.
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  workflowBuilder,
  pipe,
  loop,
  tryCatch,
  invokeWithThrow,
} from "@barnum/barnum/src/ast.js";
import {
  drop,
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
          drop<never>(),
          invokeWithThrow(stepA, throwError),
          drop(),
          invokeWithThrow(stepB, throwError),
          drop(),
          invokeWithThrow(stepC, throwError),
          done<never, string>(),
        ),
        pipe(logError, drop(), recur<never, string>()),
      ),
    ),
  )
  .run();
