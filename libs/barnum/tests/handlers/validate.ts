import { createHandler } from "../../src/core.js";
import type { LoopResult } from "../../src/core.js";

export type ValidateInput = { valid: boolean };
export type ValidateOutput = LoopResult<ValidateInput, { done: true }>;

export default createHandler<ValidateInput, ValidateOutput>({
  handle: async (input) =>
    input.valid
      ? { kind: "Break", value: { done: true } }
      : { kind: "Continue", value: { valid: false } },
});
