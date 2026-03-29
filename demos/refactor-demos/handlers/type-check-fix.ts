// Type-check/fix cycle handlers — shared between conversion and refactor workflows.
//
// typeCheck: run tsc --noEmit, return errors
// classifyErrors: split into HasErrors / Clean discriminated union for branch
// fix: fix a single type error (would invoke Claude in production)

import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

// --- Types ---

export type TypeError = {
  file: string;
  message: string;
};

export type ClassifyResult =
  | { kind: "HasErrors"; errors: TypeError[] }
  | { kind: "Clean" };

// --- Handlers ---

// In production: exec `tsc --noEmit`, parse structured output.
// Operates on the filesystem, not a pipeline value.
export const typeCheck = createHandler({
  handle: async (): Promise<TypeError[]> => {
    console.error("[type-check] Running tsc --noEmit...");
    return [];
  },
}, "typeCheck");

// Pure data transform: errors[] → { kind: "HasErrors" | "Clean" }.
export const classifyErrors = createHandler({
  inputValidator: z.array(
    z.object({ file: z.string(), message: z.string() }),
  ),
  handle: async ({ value: errors }): Promise<ClassifyResult> => {
    if (errors.length > 0) {
      console.error(`[classify-errors] Found ${errors.length} error(s)`);
      return { kind: "HasErrors", errors };
    }
    console.error("[classify-errors] Clean — no type errors");
    return { kind: "Clean" };
  },
}, "classifyErrors");

// In production:
//   Prompt: "Fix the type error in {file}: {message}. Read the file,
//   understand the issue, and make the minimal edit to resolve it."
export const fix = createHandler({
  inputValidator: z.object({
    file: z.string(),
    message: z.string(),
  }),
  handle: async ({ value: error }) => {
    console.error(`[fix] Fixing: ${error.file} — ${error.message}`);
    return { file: error.file, fixed: true as const };
  },
}, "fix");
