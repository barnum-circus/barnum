import { z } from "zod";
import { createHandler } from "../../src/core.js";

export default createHandler({
  stepValueValidator: z.object({ valid: z.boolean() }),

  handle: async () => ({ done: true as const }),
});
