import { z } from "zod";
import { createHandler } from "../../src/core.js";

export default createHandler({
  stepValueValidator: z.object({ file: z.string() }),
  handle: async ({ value }) => ({
    file: value.file,
    migrated: true,
  }),
});
