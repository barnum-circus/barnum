import { createHandler } from "@barnum/barnum/src/handler.js";
import { z } from "zod";

export const listFiles = createHandler({
  handle: async () => {
    console.error("[listFiles] Listing files...");
    return ["auth.ts", "database.ts", "routes.ts"];
  },
}, "listFiles");

export const refactor = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    console.error(`[refactor] Refactoring ${file}`);
    return file;
  },
}, "refactor");

export const typeCheck = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    console.error(`[typeCheck] Type-checking ${file}`);
    return file;
  },
}, "typeCheck");

export const fix = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }) => {
    console.error(`[fix] Fixing ${file}`);
    return file;
  },
}, "fix");

export const commit = createHandler({
  inputValidator: z.string(),
  handle: async ({ value: file }): Promise<void> => {
    console.error(`[commit] Committing ${file}`);
  },
}, "commit");
