import { z } from "zod";
import { createHandler } from "../../src/handler.js";
import type { LoopResult } from "../../src/ast.js";

export default createHandler({
  stepValueValidator: z.object({ valid: z.boolean() }),

  handle: async ({ value }): Promise<LoopResult<{ valid: boolean }, { done: true }>> =>
    value.valid
      ? { kind: "Break", value: { done: true } }
      : { kind: "Continue", value: { valid: false } },
});
