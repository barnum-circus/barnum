// Shared utilities for implement-feature demo handlers.

import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const baseDir = path.resolve(__dirname, "..");
export const srcDir = path.join(baseDir, "src");
export const outDir = path.join(baseDir, "out");
