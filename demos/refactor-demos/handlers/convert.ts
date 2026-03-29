// Handlers for the convert-folder-to-ts workflow.
//
// setup: clean output dir, return project config
// listFiles: scan input dir for source files
// migrate: convert a file to the target language (uses step config)
// writeFile: write converted content to disk

import { createHandler, createHandlerWithConfig } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

// --- Types ---

export type ProjectConfig = {
  inputDir: string;
  outputDir: string;
};

// --- Handlers ---

// In production: rm -rf out/ && mkdir -p out/.
export const setup = createHandler({
  handle: async (): Promise<ProjectConfig> => {
    console.error("[setup] Cleaning output directory, preparing project...");
    return { inputDir: "src", outputDir: "out" };
  },
}, "setup");

// In production: glob inputDir for matching files (e.g. *.js),
// compute output paths (src/foo.js → out/foo.ts).
// Returns objects (not bare strings) so downstream steps can
// parallel + merge file metadata with transformed content.
export const listFiles = createHandler({
  inputValidator: z.object({
    inputDir: z.string(),
    outputDir: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[list-files] Scanning ${value.inputDir}/ for JS files...`);
    const files = ["format.js", "greet.js", "math.js"];
    return files.map((name) => ({
      file: `${value.inputDir}/${name}`,
      outputPath: `${value.outputDir}/${name.replace(/\.js$/, ".ts")}`,
    }));
  },
}, "listFiles");

// In production:
//   Prompt: "Convert the following {from} file to {to}. Preserve all
//   behavior and add appropriate type annotations. Return only the
//   converted source code."
//   Input: file contents read from disk
//   Output: converted source code as a string
//
// The `to` parameter comes from step config (e.g. { to: 'Typescript' }).
// Pipeline value is the source file path.
export const migrate = createHandlerWithConfig({
  inputValidator: z.string(),
  stepConfigValidator: z.object({ to: z.string() }),
  handle: async ({ value: file, stepConfig }) => {
    console.error(`[migrate] Converting ${file} to ${stepConfig.to}`);
    return {
      content: `// Converted from ${file} to ${stepConfig.to}\nexport {};\n`,
    };
  },
}, "migrate");

// In production: mkdir -p parent dir, write content to outputPath.
export const writeFile = createHandler({
  inputValidator: z.object({
    content: z.string(),
    outputPath: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[write-file] → ${value.outputPath} (${value.content.length} chars)`);
    return { writtenPath: value.outputPath };
  },
}, "writeFile");
