import { z } from "zod";
import { createHandler } from "../../src/handler.js";

export default createHandler({
  stepValueValidator: z.object({ result: z.string() }),

  handle: async () => ({ valid: true }),
});
