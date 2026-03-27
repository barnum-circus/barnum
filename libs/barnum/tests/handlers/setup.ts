import { z } from "zod";
import { createHandler } from "../../src/handler.js";

export default createHandler({
  stepValueValidator: z.object({ project: z.string() }),

  handle: async ({ value }) => ({
    initialized: true,
    project: value.project,
  }),
});
