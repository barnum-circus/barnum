// ClassifyErrors: determine whether type-check found errors.
//
// Returns a discriminated union: { kind: "HasErrors", errors } or { kind: "Clean" }.
// The branch combinator dispatches on `kind`.

import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";
import type { TypeError } from "./type-check.js";

export type ClassifyResult =
  | { kind: "HasErrors"; errors: TypeError[] }
  | { kind: "Clean" };

export default createHandler({
  stepValueValidator: z.array(
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
});
