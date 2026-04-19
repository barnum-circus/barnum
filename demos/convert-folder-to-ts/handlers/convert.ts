// Handlers for the convert-folder-to-ts workflow.
//
// setup: clean output dir, return project config with absolute paths
// listFiles: scan input dir for JS source files
// migrate: invoke Claude to convert a JS file to TypeScript (reads source, writes output)

import { createHandler, createHandlerWithConfig } from "@barnum/barnum/runtime";
import { existsSync, mkdirSync, readdirSync, rmSync } from "node:fs";
import path from "node:path";
import { z } from "zod";
import { baseDir } from "./lib";
import { callClaude } from "./call-claude";

// --- Types ---

export type ProjectConfig = {
  inputDir: string;
  outputDir: string;
};

// --- Handlers ---

export const setup = createHandler(
  {
    outputValidator: z.object({ inputDir: z.string(), outputDir: z.string() }),
    handle: async (): Promise<ProjectConfig> => {
      const inputDir = path.join(baseDir, "src");
      const outputDir = path.join(baseDir, "out");

      console.error("[setup] Cleaning output directory...");
      if (existsSync(outputDir)) {
        rmSync(outputDir, { recursive: true });
      }
      mkdirSync(outputDir, { recursive: true });

      console.error(`[setup] inputDir: ${inputDir}`);
      console.error(`[setup] outputDir: ${outputDir}`);
      return { inputDir, outputDir };
    },
  },
  "setup",
);

export const listFiles = createHandler(
  {
    inputValidator: z.object({
      inputDir: z.string(),
      outputDir: z.string(),
    }),
    outputValidator: z.array(
      z.object({ file: z.string(), outputPath: z.string() }),
    ),
    handle: async ({ value }) => {
      console.error(`[list-files] Scanning ${value.inputDir}/ for JS files...`);
      const files = readdirSync(value.inputDir)
        .filter((name) => name.endsWith(".js"))
        .sort();

      const result = files.map((name) => ({
        file: path.join(value.inputDir, name),
        outputPath: path.join(value.outputDir, name.replace(/\.js$/, ".ts")),
      }));

      console.error(
        `[list-files] Found ${result.length} files: ${files.join(", ")}`,
      );
      return result;
    },
  },
  "listFiles",
);

export const migrate = createHandlerWithConfig(
  {
    inputValidator: z.object({
      file: z.string(),
      outputPath: z.string(),
    }),
    stepConfigValidator: z.object({ to: z.string() }),
    handle: async ({ value, stepConfig }) => {
      const fileName = path.basename(value.file);

      console.error(
        `[migrate] Converting ${fileName} to ${stepConfig.to} via Claude...`,
      );

      await callClaude({
        prompt: [
          `Convert the JavaScript file at ${value.file} to ${stepConfig.to}.`,
          "Add proper type annotations to all function parameters and return types.",
          "Use ES module syntax (export/import) instead of CommonJS (module.exports/require).",
          "Preserve all behavior exactly.",
          "",
          `Read the source file at: ${value.file}`,
          `Write the converted ${stepConfig.to} to: ${value.outputPath}`,
        ].join("\n"),
        allowedTools: [`Read(//${value.file})`, `Write(//${value.outputPath})`],
      });

      console.error(`[migrate] Wrote ${value.outputPath}`);
    },
  },
  "migrate",
);
