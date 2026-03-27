import { z } from "zod";
import { createHandler } from "../src/handler.js";
import type { LoopResult } from "../src/ast.js";

// ---------------------------------------------------------------------------
// setup: { project: string } → { initialized: true, project: string }
// ---------------------------------------------------------------------------

export const setup = createHandler(
  {
    stepValueValidator: z.object({ project: z.string() }),
    handle: async ({ value }) => ({
      initialized: true,
      project: value.project,
    }),
  },
  "setup",
);

// ---------------------------------------------------------------------------
// process: { initialized: boolean, project: string } → { result: string }
// ---------------------------------------------------------------------------

export const process = createHandler(
  {
    stepValueValidator: z.object({
      initialized: z.boolean(),
      project: z.string(),
    }),
    handle: async ({ value }) => ({ result: `processed ${value.project}` }),
  },
  "process",
);

// ---------------------------------------------------------------------------
// check: { result: string } → { valid: boolean }
// ---------------------------------------------------------------------------

export const check = createHandler(
  {
    stepValueValidator: z.object({ result: z.string() }),
    handle: async () => ({ valid: true }),
  },
  "check",
);

// ---------------------------------------------------------------------------
// finalize: { valid: boolean } → { done: true }
// ---------------------------------------------------------------------------

export const finalize = createHandler(
  {
    stepValueValidator: z.object({ valid: z.boolean() }),
    handle: async () => ({ done: true as const }),
  },
  "finalize",
);

// ---------------------------------------------------------------------------
// validate: { valid: boolean } → LoopResult<{ valid: boolean }, { done: true }>
// ---------------------------------------------------------------------------

export const validate = createHandler(
  {
    stepValueValidator: z.object({ valid: z.boolean() }),
    handle: async ({
      value,
    }): Promise<LoopResult<{ valid: boolean }, { done: true }>> =>
      value.valid
        ? { kind: "Break", value: { done: true } }
        : { kind: "Continue", value: { valid: false } },
  },
  "validate",
);

// ---------------------------------------------------------------------------
// listFiles: { initialized: boolean, project: string } → { file: string }[]
// ---------------------------------------------------------------------------

export const listFiles = createHandler(
  {
    stepValueValidator: z.object({
      initialized: z.boolean(),
      project: z.string(),
    }),
    handle: async ({ value }) => [
      { file: `${value.project}/src/index.ts` },
      { file: `${value.project}/src/utils.ts` },
    ],
  },
  "listFiles",
);

// ---------------------------------------------------------------------------
// migrate: { file: string } → { file: string, migrated: true }
// ---------------------------------------------------------------------------

export const migrate = createHandler(
  {
    stepValueValidator: z.object({ file: z.string() }),
    handle: async ({ value }) => ({
      file: value.file,
      migrated: true,
    }),
  },
  "migrate",
);

// ---------------------------------------------------------------------------
// typeCheck: never → TypeError[]
// ---------------------------------------------------------------------------

export type TypeError = { file: string; message: string };

export const typeCheck = createHandler(
  {
    stepValueValidator: z.never(),
    handle: async (): Promise<TypeError[]> => [
      { file: "src/index.ts", message: "Type error" },
    ],
  },
  "typeCheck",
);

// ---------------------------------------------------------------------------
// classifyErrors: TypeError[] → ClassifyResult
// ---------------------------------------------------------------------------

export type ClassifyResult =
  | { kind: "HasErrors"; errors: TypeError[] }
  | { kind: "Clean" };

export const classifyErrors = createHandler(
  {
    stepValueValidator: z.array(
      z.object({ file: z.string(), message: z.string() }),
    ),
    handle: async ({ value }): Promise<ClassifyResult> =>
      value.length > 0
        ? { kind: "HasErrors", errors: value }
        : { kind: "Clean" },
  },
  "classifyErrors",
);

// ---------------------------------------------------------------------------
// fix: { file: string, message: string } → { file: string, fixed: true }
// ---------------------------------------------------------------------------

export const fix = createHandler(
  {
    stepValueValidator: z.object({ file: z.string(), message: z.string() }),
    handle: async ({ value }) => ({ file: value.file, fixed: true }),
  },
  "fix",
);
