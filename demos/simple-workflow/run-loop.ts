/**
 * Demo: polling loop workflow.
 *
 * Pipeline: start-polling → loop(poll-status)
 *
 * The poll handler returns Continue until attempt >= 3, then Break.
 *
 * Usage: pnpm exec tsx run-loop.ts
 */

import { configBuilder, pipe, loop } from "@barnum/barnum/src/ast.js";
import startPolling from "./handlers/start-polling.js";
import pollStatus from "./handlers/poll-status.js";

console.error("=== Running polling loop workflow ===\n");

await configBuilder()
  .workflow(() => pipe(startPolling(), loop(pollStatus())))
  .run();
