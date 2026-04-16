/**
 * Babysit-PRs demo: monitor open PRs, fix CI failures, and land when green.
 *
 * Pipeline:
 *   1. For each PR: check status (fake GitHub API)
 *   2. Branch on result:
 *      - ChecksFailed → fix issues (side effect), wrap PR as Some
 *      - ChecksPassed → land the PR (side effect), wrap as None (done)
 *      - Landed → already merged, wrap as None (done)
 *   3. Option.collect() gathers remaining PR numbers (the Somes)
 *   4. If PRs remain, delay 10s then loop back to step 1
 *
 * Demonstrates: loop, forEach, branch, bindInput, Option.some,
 *               Option.collect, drop.
 *
 * Usage: pnpm run demo
 */

import {
  runPipeline,
  pipe,
  forEach,
  loop,
  drop,
  Option,
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
    pipe(
      forEach(
        bindInput<number>((prNumber) =>
          prNumber.then(checkPR).branch({
            ChecksFailed: fixIssues
              .drop()
              .then(prNumber)
              .then(Option.some<number>()),
            ChecksPassed: landPR.drop().then(Option.none<number>()),
            Landed: drop.then(Option.none<number>()),
          }),
        ),
      ),
      Option.collect<number>(),
      classifyRemaining.branch({
        HasPRs: bindInput<number[], never>((prs) =>
          sleep(10_000).then(prs).then(recur),
        ),
        AllDone: done,
      }),
    ),
  ),
  [101, 102, 103],
);
