import { z } from "zod";
import { createHandler } from "../../src/handler.js";

export default createHandler({
  stepValueValidator: z.object({
    initialized: z.boolean(),
    project: z.string(),
  }),
  handle: async ({ value }) => [
    { file: `${value.project}/src/index.ts` },
    { file: `${value.project}/src/utils.ts` },
  ],
});
