import { z } from "zod";
import { createHandler } from "../../src/core.js";

export type TypeError = { file: string; message: string };
export type ClassifyResult =
  | { kind: "HasErrors"; errors: TypeError[] }
  | { kind: "Clean" };

export default createHandler({
  stepValueValidator: z.array(
    z.object({ file: z.string(), message: z.string() }),
  ),
  handle: async ({ value }): Promise<ClassifyResult> =>
    value.length > 0
      ? { kind: "HasErrors", errors: value }
      : { kind: "Clean" },
});
