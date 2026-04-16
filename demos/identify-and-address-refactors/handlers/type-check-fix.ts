// Type-check/fix cycle handlers — shared between conversion and refactor workflows.
//
// typeCheck: run tsc --noEmit on the output directory, parse errors
// classifyErrors: split into HasErrors / Clean discriminated union for branch
// fix: invoke Claude to fix a single type error

import {
  createHandler,
  taggedUnionSchema,
} from "@barnum/barnum/runtime";
import {
  bindInput,
  pipe,
  forEach,
  loop,
} from "@barnum/barnum/pipeline";
import { spawnSync } from "node:child_process";
import { readdirSync } from "node:fs";
import path from "node:path";
import { z } from "zod";
import { baseDir } from "./lib";
import { callClaude } from "./call-claude";

// --- Types ---

export type TypeError = {
  file: string;
  message: string;
};

import type { TaggedUnion } from "@barnum/barnum/runtime";

type ClassifyResultDef = {
  HasErrors: TypeError[];
  Clean: void;
};
export type ClassifyResult = TaggedUnion<ClassifyResultDef>;

// --- Helpers ---

/** Parse tsc error output into structured errors. */
function parseTscErrors(output: string): TypeError[] {
  const errors: TypeError[] = [];
  // Match: path(line,col): error TSxxxx: message
  const pattern = /^(.+?)\((\d+),(\d+)\): error (TS\d+): (.+)$/gm;
  let match;
  while ((match = pattern.exec(output)) !== null) {
    errors.push({
      file: match[1],
      message: `${match[4]}: ${match[5]} (line ${match[2]})`,
    });
  }
  return errors;
}

// --- Validators ---

const TypeErrorValidator = z.object({ file: z.string(), message: z.string() });

// --- Handlers ---

export const typeCheck = createHandler({
  outputValidator: z.array(TypeErrorValidator),
  handle: async (): Promise<TypeError[]> => {
    const outDir = path.join(baseDir, "out");
    console.error(`[type-check] Running tsc --noEmit on ${outDir}...`);

    // Find all .ts files in the output directory
    const tsFiles = readdirSync(outDir)
      .filter((f) => f.endsWith(".ts"))
      .map((f) => path.join(outDir, f));

    if (tsFiles.length === 0) {
      console.error("[type-check] No .ts files found");
      return [];
    }

    const tscPath = path.join(baseDir, "node_modules", ".bin", "tsc");
    const result = spawnSync(tscPath, [
      "--noEmit", "--strict", "--esModuleInterop",
      "--target", "ES2020", "--module", "ES2020", "--moduleResolution", "node",
      ...tsFiles,
    ], {
      encoding: "utf-8",
      cwd: baseDir,
      timeout: 30_000,
    });

    const output = result.stdout + result.stderr;
    const errors = parseTscErrors(output);

    if (errors.length > 0) {
      console.error(`[type-check] Found ${errors.length} error(s)`);
      for (const error of errors) {
        console.error(`  ${error.file}: ${error.message}`);
      }
    } else {
      console.error("[type-check] Clean — no type errors");
    }

    return errors;
  },
}, "typeCheck");

export const classifyErrors = createHandler({
  inputValidator: z.array(TypeErrorValidator),
  outputValidator: taggedUnionSchema({ HasErrors: z.array(TypeErrorValidator), Clean: z.null() }),
  handle: async ({ value: errors }): Promise<ClassifyResult> => {
    console.error(`[classify-errors] Called with ${errors.length} error(s)`);
    if (errors.length > 0) {
      console.error(`[classify-errors] ${errors.length} error(s) to fix`);
      return { kind: "HasErrors", value: errors };
    }
    console.error("[classify-errors] Clean — no type errors");
    return { kind: "Clean", value: null };
  },
}, "classifyErrors");

export const fix = createHandler({
  inputValidator: TypeErrorValidator,
  outputValidator: z.object({ file: z.string(), fixed: z.literal(true) }),
  handle: async ({ value: error }) => {
    console.error(`[fix] Asking Claude to fix: ${error.file} — ${error.message}`);

    await callClaude({
      prompt: [
        `Fix this TypeScript type error:`,
        `File: ${error.file}`,
        `Error: ${error.message}`,
        "",
        "Read the file, understand the issue, and edit it to fix the error.",
        "Make the minimal change needed. Do not change behavior.",
      ].join("\n"),
      allowedTools: ["Read", "Edit"],
      cwd: baseDir,
    });

    console.error(`[fix] Applied fix to ${error.file}`);
    return { file: error.file, fixed: true as const };
  },
}, "fix");

// --- Pipeline ---

export const typeCheckFix = bindInput<{ worktreePath: string }>((typeCheckFixParams) =>
  loop<void, void>((recur, done) =>
    typeCheckFixParams.then(pipe(typeCheck, classifyErrors)).branch({
      HasErrors: forEach(fix).drop().then(recur),
      Clean: done,
    }),
  ),
);
