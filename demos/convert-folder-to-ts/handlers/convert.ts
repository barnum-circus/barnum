// Handlers for the convert-folder-to-ts workflow.
//
// setup: clean output dir, return project config with absolute paths
// listFiles: scan input dir for JS source files
// migrate: invoke Claude to convert a JS file to TypeScript
// writeFile: write converted content to disk

import { createHandler, createHandlerWithConfig } from "@barnum/barnum/src/handler.js";
import { existsSync, mkdirSync, readdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { z } from "zod";
import { baseDir, callClaude, stripCodeFences } from "./lib.js";

// --- Types ---

export type ProjectConfig = {
  inputDir: string;
  outputDir: string;
};

// --- Handlers ---

export const setup = createHandler({
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
}, "setup");

export const listFiles = createHandler({
  inputValidator: z.object({
    inputDir: z.string(),
    outputDir: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[list-files] Scanning ${value.inputDir}/ for JS files...`);
    const files = readdirSync(value.inputDir)
      .filter((name) => name.endsWith(".js"))
      .sort();

    const result = files.map((name) => ({
      file: path.join(value.inputDir, name),
      outputPath: path.join(value.outputDir, name.replace(/\.js$/, ".ts")),
    }));

    console.error(`[list-files] Found ${result.length} files: ${files.join(", ")}`);
    return result;
  },
}, "listFiles");

export const migrate = createHandlerWithConfig({
  inputValidator: z.string(),
  stepConfigValidator: z.object({ to: z.string() }),
  handle: async ({ value: filePath, stepConfig }) => {
    const source = readFileSync(filePath, "utf-8");
    const fileName = path.basename(filePath);

    console.error(`[migrate] Converting ${fileName} to ${stepConfig.to} via Claude...`);

    const response = callClaude({
      prompt: [
        `Convert this JavaScript file to ${stepConfig.to}.`,
        "Add proper type annotations to all function parameters and return types.",
        "Use ES module syntax (export/import) instead of CommonJS (module.exports/require).",
        "Preserve all behavior exactly. Return ONLY the converted code, no markdown fences, no explanations.",
        "",
        `File: ${fileName}`,
        "```",
        source,
        "```",
      ].join("\n"),
    });

    const content = stripCodeFences(response);
    console.error(`[migrate] ${fileName}: ${content.split("\n").length} lines of ${stepConfig.to}`);
    return { content };
  },
}, "migrate");

export const writeFile = createHandler({
  inputValidator: z.object({
    content: z.string(),
    outputPath: z.string(),
  }),
  handle: async ({ value }) => {
    console.error(`[write-file] Writing ${value.outputPath}...`);
    const dir = path.dirname(value.outputPath);
    if (!existsSync(dir)) {
      mkdirSync(dir, { recursive: true });
    }
    writeFileSync(value.outputPath, value.content, "utf-8");
    console.error(`[write-file] Wrote ${value.outputPath} (${value.content.length} chars)`);
    return { writtenPath: value.outputPath };
  },
}, "writeFile");
