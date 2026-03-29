import { z } from "zod";
import { createHandler } from "../src/handler.js";
import type { LoopResult } from "../src/ast.js";

// ---------------------------------------------------------------------------
// CI/CD pipeline handlers
// ---------------------------------------------------------------------------

// setup: { project: string } → { initialized: boolean, project: string }
export const setup = createHandler(
  {
    inputValidator: z.object({ project: z.string() }),
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
    inputValidator: z.object({
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
    inputValidator: z.object({ artifact: z.string() }),
    handle: async () => ({ verified: true }),
  },
  "verify",
);

// deploy: { verified: boolean } → { deployed: true }
export const deploy = createHandler(
  {
    inputValidator: z.object({ verified: z.boolean() }),
    handle: async (): Promise<{ deployed: boolean }> => ({ deployed: true }),
  },
  "deploy",
);

// healthCheck: { deployed: boolean } → LoopResult<{ deployed: boolean }, { stable: true }>
export const healthCheck = createHandler(
  {
    inputValidator: z.object({ deployed: z.boolean() }),
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
    inputValidator: z.object({
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
    inputValidator: z.object({ file: z.string() }),
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
    inputValidator: z.never(),
    handle: async (): Promise<TypeError[]> => [
      { file: "src/index.ts", message: "Type error" },
    ],
  },
  "typeCheck",
);

// classifyErrors: TypeError[] → ClassifyResult
export type ClassifyResult =
  | { kind: "HasErrors"; value: TypeError[] }
  | { kind: "Clean"; value: void };

export const classifyErrors = createHandler(
  {
    inputValidator: z.array(
      z.object({ file: z.string(), message: z.string() }),
    ),
    handle: async ({ value }): Promise<ClassifyResult> =>
      value.length > 0
        ? { kind: "HasErrors", value }
        : { kind: "Clean", value: undefined },
  },
  "classifyErrors",
);

// fix: { file: string, message: string } → { file: string, fixed: boolean }
export const fix = createHandler(
  {
    inputValidator: z.object({ file: z.string(), message: z.string() }),
    handle: async ({ value }) => ({ file: value.file, fixed: true }),
  },
  "fix",
);
