// Fix: fix a single type error.
//
// In a real workflow this would invoke Claude to read the error,
// find the file, and edit it to fix the type issue. Here we simulate it.

import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

export default createHandler({
  stepValueValidator: z.object({
    file: z.string(),
    message: z.string(),
  }),
  handle: async ({ value: error }) => {
    console.error(`[fix] Fixing: ${error.file} — ${error.message}`);
    return { file: error.file, fixed: true as const };
  },
});
