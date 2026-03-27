import { createHandler } from "../../src/core.js";

export type FinalizeInput = { valid: boolean };
export type FinalizeOutput = { done: true };

export default createHandler<FinalizeInput, FinalizeOutput>({
  handle: async () => ({ done: true }),
});
