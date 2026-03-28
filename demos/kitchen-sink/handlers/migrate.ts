// Migrate: convert a single JS file to TypeScript.
//
// In a real workflow this would invoke Claude to read the JS file,
// add type annotations, and write the .ts output. Here we simulate it.

import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

export default createHandler({
  stepValueValidator: z.string(),
  handle: async ({ value: file }) => {
    const tsFile = file.replace(/\.js$/, ".ts").replace(/^src\//, "out/");
    console.error(`[migrate] ${file} → ${tsFile}`);
    return { file: tsFile, migrated: true as const };
  },
});
