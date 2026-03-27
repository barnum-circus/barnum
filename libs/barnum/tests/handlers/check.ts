import { z } from "zod";
import { createHandler } from "../../src/core.js";

export default createHandler({
  stepValueValidator: z.object({ result: z.string() }),

  handle: async () => ({ valid: true }),
});
