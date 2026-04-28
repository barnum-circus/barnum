/**
 * Implement-feature demo: automated feature implementation with
 * parallel review, static analysis, and feedback loop.
 *
 * Pipeline:
 *   1. Implement the feature (agent)
 *   2. Loop until clean:
 *      a. Run all reviews and checks in parallel (allObject)
 *      b. If any issues → incorporate feedback → loop
 *      c. If all clean → done
 *   3. Every step retried up to 3 times (withRetry)
 *
 * Demonstrates: allObject, loop, tryCatch, bindInput, branch,
 * withRetry as a higher-order combinator.
 *
 * Usage: pnpm exec tsx run.ts
 */

import { runPipeline, pipe, allObject, loop } from "@barnum/barnum/pipeline";
import {
  implement,
  reviewSecurity,
  reviewQuality,
  reviewAdherence,
  runTypecheck,
  runLint,
  runTests,
  classifyFeedback,
  incorporateFeedback,
} from "./handlers/steps";
import { withRetry } from "./handlers/with-retry";

console.error("=== Implement feature demo ===\n");

runPipeline(
  pipe(
    withRetry(3, implement).drop(),

    loop((recur, done) =>
      pipe(
        allObject({
          security: withRetry(3, reviewSecurity),
          quality: withRetry(3, reviewQuality),
          adherence: withRetry(3, reviewAdherence),
          typecheck: withRetry(3, runTypecheck),
          lint: withRetry(3, runLint),
          tests: withRetry(3, runTests),
        }),
        classifyFeedback.branch({
          HasIssues: withRetry(3, incorporateFeedback).drop().then(recur),
          AllClean: done,
        }),
      ),
    ),
  ),
  "Add a caching layer to the API endpoints",
);
