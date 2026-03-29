import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

export default createHandler({
  inputValidator: z.object({
    project: z.string(),
    version: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[build] Building ${value.project}@${value.version}...`);
    return {
      artifact: `${value.project}-${value.version}.tar.gz`,
      size: 1024,
    };
  },
});
