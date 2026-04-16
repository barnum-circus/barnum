import { createHandler, taggedUnionSchema } from "@barnum/barnum/runtime";
import { z } from "zod";

/**
 * Classify a number as zero or non-zero. Returns a tagged union
 * so the caller can branch on it.
 */
export const classifyZero = createHandler({
  inputValidator: z.number(),
  outputValidator: taggedUnionSchema("Nat", { Zero: z.null(), NonZero: z.number() }),
  handle: async ({ value: n }) => {
    if (n === 0) {
      return { kind: "Nat.Zero" as const, value: null };
    }
    return { kind: "Nat.NonZero" as const, value: n };
  },
}, "classifyZero");

/**
 * Subtract one from the input.
 */
export const subtractOne = createHandler({
  inputValidator: z.number(),
  outputValidator: z.number(),
  handle: async ({ value: n }) => n - 1,
}, "subtractOne");
