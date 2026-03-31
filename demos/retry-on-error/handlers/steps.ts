/**
 * Mock fallible handlers for the retry-on-error demo.
 *
 * Each handler returns Result<string, string> — Ok on success, Err with a
 * message on failure. Outcomes are random to demonstrate retry behavior.
 */

import { createHandler } from "@barnum/barnum/src/handler.js";
import type { Result } from "@barnum/barnum/src/ast.js";
import { z } from "zod";

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

type StepResult = Result<string, string>;

function ok(value: string): StepResult {
  return { kind: "Ok", value };
}

function err(message: string): StepResult {
  return { kind: "Err", value: message };
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/** Step A: validate input. Succeeds ~70% of the time. */
export const stepA = createHandler({
  handle: async (): Promise<StepResult> => {
    const succeed = Math.random() < 0.7;
    if (succeed) {
      console.error("[stepA] Validation passed");
      return ok("validated");
    }
    console.error("[stepA] Validation failed");
    return err("stepA: validation failed");
  },
}, "stepA");

/** Step B: process data. Succeeds ~70% of the time. */
export const stepB = createHandler({
  handle: async (): Promise<StepResult> => {
    const succeed = Math.random() < 0.7;
    if (succeed) {
      console.error("[stepB] Processing complete");
      return ok("processed");
    }
    console.error("[stepB] Processing error");
    return err("stepB: processing error");
  },
}, "stepB");

/** Step C: finalize. Succeeds ~70% of the time. */
export const stepC = createHandler({
  handle: async (): Promise<StepResult> => {
    const succeed = Math.random() < 0.7;
    if (succeed) {
      console.error("[stepC] Finalized");
      return ok("finalized");
    }
    console.error("[stepC] Finalization failed");
    return err("stepC: finalization failed");
  },
}, "stepC");

/** Log an error and prepare for retry. Receives the error message as input. */
export const logError = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: errorMessage }): Promise<void> => {
    console.error(`[logError] Error: ${errorMessage}`);
    console.error("[logError] Retrying...\n");
  },
}, "logError");
