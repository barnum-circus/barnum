/**
 * Babysit-PRs demo: monitor open PRs, fix CI failures, and land when green.
 *
 * Pipeline:
 *   1. For each PR: check status (fake GitHub API)
 *   2. Branch on result:
 *      - ChecksFailed → fix issues (side effect), keep PR (true)
 *      - ChecksPassed → land the PR (side effect), drop PR (false)
 *      - Landed → already merged, drop PR (false)
 *   3. Iterator.filter keeps PRs that still need attention
 *   4. If PRs remain, delay 10s then loop back to step 1
 *
 * Demonstrates: loop, Iterator.filter, branch, bindInput, constant, drop.
 *
 * Usage: pnpm run demo
 */

import {
  runPipeline,
  Iterator,
  constant,
  loop,
  drop,
  bindInput,
  sleep,
} from "@barnum/barnum/pipeline";
import {
  checkPR,
  fixIssues,
  landPR,
  classifyRemaining,
} from "./handlers/steps";

console.error("=== Babysit PRs demo ===\n");
console.error("Monitoring PRs: #101, #102, #103\n");

runPipeline(
  loop<void, number[]>((recur, done) =>
    Iterator.fromArray<number>()
      .filter(
        bindInput<number>((prNumber) =>
          prNumber.then(checkPR).branch({
            ChecksFailed: fixIssues.drop().then(constant(true)),
            ChecksPassed: landPR.drop().then(constant(false)),
            Landed: drop.then(constant(false)),
          }),
        ),
      )
      .collect()
      .then(classifyRemaining)
      .branch({
        HasPRs: bindInput<number[], never>((prs) =>
          sleep(10_000).then(prs).then(recur),
        ),
        AllDone: done,
      }),
  ),
  [101, 102, 103],
);
