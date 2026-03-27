import { z } from "zod";
import { createHandler } from "../src/handler.js";
import type { LoopResult } from "../src/ast.js";

// ---------------------------------------------------------------------------
// CI/CD pipeline handlers
// ---------------------------------------------------------------------------

// setup: { project: string } → { initialized: boolean, project: string }
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

// build: { initialized: boolean, project: string } → { artifact: string }
export const build = createHandler(
  {
    stepValueValidator: z.object({
      initialized: z.boolean(),
      project: z.string(),
    }),
    handle: async ({ value }) => ({ artifact: `${value.project}.build` }),
  },
  "build",
);

// verify: { artifact: string } → { verified: boolean }
export const verify = createHandler(
  {
    stepValueValidator: z.object({ artifact: z.string() }),
    handle: async () => ({ verified: true }),
  },
  "verify",
);

// deploy: { verified: boolean } → { deployed: true }
export const deploy = createHandler(
  {
    stepValueValidator: z.object({ verified: z.boolean() }),
    handle: async () => ({ deployed: true as const }),
  },
  "deploy",
);

// healthCheck: { deployed: boolean } → LoopResult<{ deployed: boolean }, { stable: true }>
export const healthCheck = createHandler(
  {
    stepValueValidator: z.object({ deployed: z.boolean() }),
    handle: async ({
      value,
    }): Promise<LoopResult<{ deployed: boolean }, { stable: true }>> =>
      value.deployed
        ? { kind: "Break", value: { stable: true } }
        : { kind: "Continue", value: { deployed: false } },
  },
  "healthCheck",
);

// ---------------------------------------------------------------------------
// Migration handlers
// ---------------------------------------------------------------------------

// listFiles: { initialized: boolean, project: string } → { file: string }[]
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

// migrate: { file: string } → { file: string, migrated: boolean }
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

// typeCheck: never → TypeError[]
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

// classifyErrors: TypeError[] → ClassifyResult
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

// fix: { file: string, message: string } → { file: string, fixed: boolean }
export const fix = createHandler(
  {
    stepValueValidator: z.object({ file: z.string(), message: z.string() }),
    handle: async ({ value }) => ({ file: value.file, fixed: true }),
  },
  "fix",
);
