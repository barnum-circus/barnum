// Setup: clean the output directory and return the project config.
//
// In a real workflow this would rm -rf out/ and mkdir -p out/.
// Here we just return the directory paths.

import { createHandler } from "@barnum/barnum/src/handler.js";

export type ProjectConfig = {
  inputDir: string;
  outputDir: string;
};

export default createHandler({
  handle: async (): Promise<ProjectConfig> => {
    console.error("[setup] Cleaning output directory, preparing project...");
    return { inputDir: "src", outputDir: "out" };
  },
});
