import { z } from "zod";
import { createHandler } from "../../src/core.js";

export default createHandler({
  stepValueValidator: z.object({
    initialized: z.boolean(),
    project: z.string(),
  }),

  handle: async ({ value }) => ({ result: `processed ${value.project}` }),
});
