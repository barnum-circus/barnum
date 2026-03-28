import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

export default createHandler({
  stepValueValidator: z.object({
    deployed: z.boolean(),
    url: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[notify] Sending notification for ${value.url}...`);
    return { notified: true, channel: "slack" };
  },
});
