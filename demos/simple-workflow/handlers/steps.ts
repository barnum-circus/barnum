import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

export const listFiles = createHandler({
  handle: async () => {
    console.error("[listFiles] Listing files...");
    return ["auth.ts", "database.ts", "routes.ts"];
  },
}, "listFiles");

export const implementRefactor = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    console.error(`[implementRefactor] Refactoring ${file}`);
    return file;
  },
}, "implementRefactor");

export const typeCheckFiles = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    console.error(`[typeCheckFiles] Type-checking ${file}`);
    return file;
  },
}, "typeCheckFiles");

export const fixTypeErrors = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    console.error(`[fixTypeErrors] Fixing ${file}`);
    return file;
  },
}, "fixTypeErrors");

export const createPullRequest = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    console.error(`[createPullRequest] Creating PR for ${file}`);
    return file;
  },
}, "createPullRequest");

export const commitChanges = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    console.error(`[commitChanges] Committing ${file}`);
    return file;
  },
}, "commitChanges");
