import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

export default createHandler({
  inputValidator: z.object({
    artifact: z.string(),
    size: z.number(),
  }),
  handle: async ({ value }) => {
    console.error(`[deploy] Deploying ${value.artifact} (${value.size} bytes)...`);
    return {
      deployed: true,
      url: `https://example.com/${value.artifact}`,
    };
  },
});
