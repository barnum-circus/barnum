/**
 * Mock fallible handlers for the retry-on-error demo.
 *
 * Each handler returns Result<string, string> — Ok on success, Err with a
 * message on failure. Outcomes are random to demonstrate retry behavior.
 * Step B also occasionally takes a long time, demonstrating timeout handling.
 */

import { createHandler } from "@barnum/barnum";
import type { Result } from "@barnum/barnum";
import { z } from "zod";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type StepResult = Result<string, string>;

const StepResultValidator = z.union([
  z.object({ kind: z.literal("Ok"), value: z.string() }),
  z.object({ kind: z.literal("Err"), value: z.string() }),
]);

function ok(value: string): StepResult {
  return { kind: "Ok", value };
}

function err(message: string): StepResult {
  return { kind: "Err", value: message };
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/** Step A: validate input. Succeeds ~70%. Failures are catastrophic — the workflow exits. */
export const stepA = createHandler({
  outputValidator: StepResultValidator,
  handle: async (): Promise<StepResult> => {
    const succeed = Math.random() < 0.7;
    if (succeed) {
      console.error("[stepA] Validation passed");
      return ok("validated");
    }
    console.error("[stepA] CATASTROPHIC validation failure");
    return err("stepA: catastrophic validation failure — do not retry");
  },
}, "stepA");

/** Step B: process data. Succeeds ~60%, fails ~20%, hangs ~20%. */
export const stepB = createHandler({
  outputValidator: StepResultValidator,
  handle: async (): Promise<StepResult> => {
    const roll = Math.random();
    if (roll < 0.2) {
      // Simulate a slow operation that will be killed by timeout
      console.error("[stepB] Processing... (slow)");
      await new Promise((resolve) => setTimeout(resolve, 30_000));
      return ok("processed"); // unreachable if timeout fires
    }
    if (roll < 0.4) {
      console.error("[stepB] Processing error");
      return err("stepB: processing error");
    }
    console.error("[stepB] Processing complete");
    return ok("processed");
  },
}, "stepB");

/** Step C: finalize. Succeeds ~80%, fails ~20%. Failures are retried. */
export const stepC = createHandler({
  outputValidator: StepResultValidator,
  handle: async (): Promise<StepResult> => {
    const succeed = Math.random() < 0.8;
    if (succeed) {
      console.error("[stepC] Finalized");
      return ok("finalized");
    }
    console.error("[stepC] Finalization failed");
    return err("stepC: finalization failed");
  },
}, "stepC");

/** Log an error and prepare for retry. Receives the error string. */
export const logError = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: error }): Promise<void> => {
    console.error(`[logError] Error: ${error}`);
    console.error("[logError] Retrying...\n");
  },
}, "logError");
