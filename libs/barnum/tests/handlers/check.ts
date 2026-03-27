import { createHandler } from "../../src/core.js";

export type CheckInput = { result: string };
export type CheckOutput = { valid: boolean };

export default createHandler<CheckInput, CheckOutput>({
  handle: async () => ({ valid: true }),
});
