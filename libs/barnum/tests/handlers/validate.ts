import { createHandler } from "../../src/core.js";

export type ValidateInput = { valid: boolean };
export type ValidateOutput = { valid: boolean };

export default createHandler<ValidateInput, ValidateOutput>({
  handle: async (input) => ({ valid: input.valid }),
});
