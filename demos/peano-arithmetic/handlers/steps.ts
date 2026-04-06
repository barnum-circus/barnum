import { createHandler } from "@barnum/barnum";
import { z } from "zod";

/**
 * Classify a number as zero or non-zero. Returns a tagged union
 * so the caller can branch on it.
 */
export const classifyZero = createHandler({
  inputValidator: z.number(),
  outputValidator: z.discriminatedUnion("kind", [
    z.object({ kind: z.literal("Zero"), value: z.void() }),
    z.object({ kind: z.literal("NonZero"), value: z.number() }),
  ]),
  handle: async ({ value: n }) => {
    if (n === 0) {
      return { kind: "Zero" as const, value: undefined };
    }
    return { kind: "NonZero" as const, value: n };
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
