import { createHandler } from "../../src/core.js";

export type ProcessInput = { initialized: boolean; project: string };
export type ProcessOutput = { result: string };

export default createHandler<ProcessInput, ProcessOutput>({
  handle: async (input) => ({ result: `processed ${input.project}` }),
});
