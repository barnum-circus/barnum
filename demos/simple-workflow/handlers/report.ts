import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

export default createHandler({
  inputValidator: z.object({
    deployed: z.boolean(),
    url: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[report] Deployment complete: ${value.url}`);
    return { status: "success", url: value.url };
  },
});
