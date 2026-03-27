import { z } from "zod";
import { createHandler } from "../../src/handler.js";

export default createHandler({
  stepValueValidator: z.object({ file: z.string() }),
  handle: async ({ value }) => ({
    file: value.file,
    migrated: true,
  }),
});
