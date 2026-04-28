/**
 * Implement-feature demo: automated feature implementation with
 * parallel review, static analysis, and feedback loop.
 *
 * Pipeline:
 *   1. Setup: clear out/, copy src/ to out/
 *   2. Implement the feature (agent)
 *   3. Review loop (max 3 iterations):
 *      a. Run all checks in parallel (allObject)
 *      b. If any issues → incorporate feedback → loop
 *      c. If all clean → done
 *   4. Split into commits (no-op)
 *
 * Each agent step retried up to 3 times (withRetry).
 *
 * Demonstrates: allObject, loop, earlyReturn, tryCatch, bindInput,
 * branch, withRetry and withMaxAttempts as higher-order combinators.
 *
 * Usage: pnpm exec tsx run.ts
 */

import {
  runPipeline,
  pipe,
  allObject,
  bindInput,
  drop,
} from "@barnum/barnum/pipeline";
import {
  setup,
  implement,
  reviewBestPractices,
  reviewAdherence,
  checkSuppressedTests,
  runTypecheck,
  classifyFeedback,
  incorporateFeedback,
  splitCommits,
} from "./handlers/steps";
import { withRetry } from "./handlers/with-retry";
import { withMaxAttempts } from "./handlers/with-max-attempts";

console.error("=== Implement feature demo ===\n");

const DESCRIPTION =
  "Replace the text input in SearchPage.tsx with a debounced autocomplete. " +
  "Use the existing fetchSuggestions function from autocomplete.ts. " +
  "Debounce at 300ms. Show suggestions in a dropdown below the input.";

runPipeline(
  bindInput<string>((description) =>
    pipe(
      setup,
      description.then(withRetry(3, implement)).drop(),

      withMaxAttempts<void>(3, (recur, done) =>
        pipe(
          allObject({
            bestPractices: withRetry(3, reviewBestPractices),
            adherence: description.then(withRetry(3, reviewAdherence)),
            suppressedTests: withRetry(3, checkSuppressedTests),
            typecheck: withRetry(3, runTypecheck),
          }),
          classifyFeedback.branch({
            HasIssues: bindInput<string, never>((feedback) =>
              allObject({
                description,
                feedback,
              })
                .then(withRetry(3, incorporateFeedback))
                .drop()
                .then(recur),
            ),
            AllClean: drop.then(done),
          }),
        ),
      ),

      splitCommits.drop(),
    ),
  ),
  DESCRIPTION,
);
