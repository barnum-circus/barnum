import { z } from "zod";
import { createHandler } from "../../src/handler.js";

export default createHandler({
  stepValueValidator: z.object({ valid: z.boolean() }),

  handle: async () => ({ done: true as const }),
});
