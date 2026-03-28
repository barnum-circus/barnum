// ListFiles: scan the input directory for JS files to migrate.
//
// In a real workflow this would glob src/*.js.
// Here we return hardcoded file paths matching the demo src/ files.

import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

export default createHandler({
  stepValueValidator: z.object({
    inputDir: z.string(),
    outputDir: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[list-files] Scanning ${value.inputDir}/ for JS files...`);
    return [
      `${value.inputDir}/format.js`,
      `${value.inputDir}/greet.js`,
      `${value.inputDir}/math.js`,
    ];
  },
});
