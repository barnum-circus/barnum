// Shared utilities for demo handlers.

import path from "node:path";
import { fileURLToPath } from "node:url";

const __dirname = path.dirname(fileURLToPath(import.meta.url));
export const baseDir = path.resolve(__dirname, "..");

/** Strip markdown code fences if present. */
export function stripCodeFences(text: string): string {
  const fenced = text.match(/^```(?:typescript|ts)?\n([\s\S]*?)\n```$/m);
  if (fenced) {
    return fenced[1];
  }
  return text.trim();
}
