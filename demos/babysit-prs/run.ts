/**
 * Babysit-PRs demo: monitor open PRs, fix CI failures, and land when green.
 *
 * Pipeline:
 *   1. For each PR: check status (fake GitHub API, random delay)
 *   2. Branch on result:
 *      - ChecksFailed → fix issues (fake LLM), return PR number for retry
 *      - ChecksPassed → land the PR, return null (done)
 *      - Landed → already merged, drop (done)
 *   3. Filter out nulls; if PRs remain, loop back to step 1
 *
 * Demonstrates: loop, forEach, branch, drop.
 *
 * Usage: pnpm run demo
 */

import { runPipeline, pipe, forEach, loop, drop } from "@barnum/barnum";
import {
  checkPR,
  fixIssues,
  landPR,
  classifyRemaining,
} from "./handlers/steps.js";

console.error("=== Babysit PRs demo ===\n");
console.error("Monitoring PRs: #101, #102, #103\n");

runPipeline(
  loop<void, number[]>((recur, done) =>
    pipe(
      forEach(
        checkPR.branch({
          ChecksFailed: fixIssues,
          ChecksPassed: landPR,
          Landed: drop,
        }),
      ),
      classifyRemaining.branch({
        HasPRs: recur,
        AllDone: done,
      }),
    ),
  ),
  [101, 102, 103],
);
