import { createHandler } from "@barnum/barnum/runtime";
import { z } from "zod";

const randomDelay = (): Promise<void> =>
  new Promise<void>((resolve) =>
    setTimeout(resolve, Math.floor(Math.random() * 1000)),
  );

export const listFiles = createHandler(
  {
    outputValidator: z.array(z.string()),
    handle: async () => {
      await randomDelay();
      console.error("[listFiles] Listing files...");
      return ["auth.ts", "database.ts", "routes.ts"];
    },
  },
  "listFiles",
);

export const implementRefactor = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: file }) => {
      await randomDelay();
      console.error(`[implementRefactor] Refactoring ${file}`);
      return file;
    },
  },
  "implementRefactor",
);

export const typeCheckFiles = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: file }) => {
      await randomDelay();
      console.error(`[typeCheckFiles] Type-checking ${file}`);
      return file;
    },
  },
  "typeCheckFiles",
);

export const fixTypeErrors = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: file }) => {
      await randomDelay();
      console.error(`[fixTypeErrors] Fixing ${file}`);
      return file;
    },
  },
  "fixTypeErrors",
);

export const createPullRequest = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: file }) => {
      await randomDelay();
      console.error(`[createPullRequest] Creating PR for ${file}`);
      return file;
    },
  },
  "createPullRequest",
);

export const commitChanges = createHandler(
  {
    inputValidator: z.string(),
    outputValidator: z.string(),
    handle: async ({ value: file }) => {
      await randomDelay();
      console.error(`[commitChanges] Committing ${file}`);
      return file;
    },
  },
  "commitChanges",
);
