import { z } from "zod";
import { createHandler } from "../../src/handler.js";
import type { TypeError } from "./classify-errors.js";

export default createHandler({
  stepValueValidator: z.unknown(),
  handle: async (): Promise<TypeError[]> => [
    { file: "src/index.ts", message: "Type error" },
  ],
});
